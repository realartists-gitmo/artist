//! Read-only repository projections for `repo://` resources.
//!
//! The vendored `artist-ast` fork supplies structured multi-language
//! declarations. This adapter translates those declarations into repository
//! resources and Teca-compatible file anchors. It is deliberately read-only:
//! source edits remain native-path `edit` requests, anchored by the kernel.

use crate::{
    AnchorSet, AnchoredLine, AnchoredText, BoxFuture, Handler, HandlerDescriptor, KernelError,
    KernelHandle, Operation, OperationResult, ReadResult, Request, ResourceAddress,
    StructuralAnalyzer, StructuralLine, TypedHandler, Verb, fff, is_file_uri, normalize,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use url::Url;

pub struct RepositoryHandler {
    root: PathBuf,
    project: String,
    structure: StructuralAnalyzer,
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
        })
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
        Ok(serde_json::from_str(&artist_ast::core::render_json_map(
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

impl Handler for RepositoryHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            name: "repository".to_owned(),
            schemes: vec!["repo".to_owned(), "file".to_owned()],
            verbs: vec![Verb::Read, Verb::Find, Verb::Grep],
        }
    }

    fn claims(&self, address: &ResourceAddress) -> bool {
        address
            .as_uri()
            .is_some_and(|uri| uri.scheme() == "repo" || uri.scheme() == "file")
    }

    fn execute<'a>(
        &'a self,
        request: Request,
        _host: KernelHandle,
    ) -> BoxFuture<'a, Result<Value, KernelError>> {
        Box::pin(async move {
            let target = normalize(&request.target)?;
            match request.verb {
                Verb::Read => self.read_resource(&target),
                Verb::Find => Ok(json!({"paths": self.find_paths(&target)?})),
                Verb::Grep => self.grep(&target, &request.args),
                verb => Err(KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: request.target.to_string(),
                }),
            }
        })
    }
}

impl TypedHandler for RepositoryHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            name: "repository-typed".to_owned(),
            schemes: vec!["repo".to_owned(), "file".to_owned()],
            verbs: vec![Verb::Read, Verb::Find, Verb::Grep],
        }
    }

    fn claims_operation(&self, operation: &Operation) -> bool {
        match operation {
            Operation::Read(requests) => requests.iter().all(|request| {
                matches!(request.uri.scheme(), "repo" | "file")
                    && self
                        .file_and_suffix(&ResourceAddress::uri(request.uri.clone()))
                        .is_ok()
            }),
            Operation::Find(request) => request
                .roots
                .iter()
                .all(|uri| matches!(uri.scheme(), "repo" | "file")),
            Operation::Grep(request) => matches!(
                &request.source,
                crate::GrepSource::Resources(uris)
                    if !uris.is_empty()
                        && uris.iter().all(|uri| matches!(uri.scheme(), "repo" | "file"))
            ),
            _ => false,
        }
    }

    fn execute_typed<'a>(
        &'a self,
        operation: Operation,
        _host: KernelHandle,
    ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
        Box::pin(async move {
            match operation {
                Operation::Read(requests) => {
                    let results = requests
                        .into_iter()
                        .map(|request| {
                            let target = ResourceAddress::uri(request.uri.clone());
                            let (file, suffix) = self.file_and_suffix(&target)?;
                            if !suffix.is_empty() {
                                return Err(KernelError::WrongKind {
                                    message: "typed repository read requires a source file"
                                        .to_owned(),
                                });
                            }
                            let source = fs::read(&file).map_err(|error| KernelError::Handler {
                                message: format!("read {}: {error}", file.display()),
                            })?;
                            Ok(ReadResult::Text(repository_anchored_text(
                                request.uri,
                                &file,
                                &source,
                                &self.structure,
                            )?))
                        })
                        .collect();
                    Ok(OperationResult::Read(results))
                }
                Operation::Find(request) => {
                    let mut paths = Vec::new();
                    for root in request.roots {
                        let target = ResourceAddress::uri(root);
                        paths.extend(self.find_paths(&target)?);
                    }
                    paths.sort();
                    paths.dedup();
                    Ok(OperationResult::Find(
                        paths
                            .into_iter()
                            .map(|path| crate::ResourceUri::parse(&path))
                            .collect::<Result<Vec<_>, _>>(),
                    ))
                }
                Operation::Grep(request) => {
                    let crate::GrepSource::Resources(uris) = request.source else {
                        return Err(KernelError::InvalidRequest {
                            message: "repository grep requires resource sources".to_owned(),
                        });
                    };
                    let mut matches = Vec::new();
                    for uri in uris {
                        let target = ResourceAddress::uri(uri.clone());
                        let root = self.file_and_suffix(&target)?.0;
                        for item in fff::grep(&root, &request.pattern, &self.structure)? {
                            let item_uri = if uri.scheme() == "repo" {
                                crate::ResourceUri::parse(&format!(
                                    "repo://{}/{}",
                                    self.project,
                                    relative(
                                        &self.root,
                                        &item
                                            .uri
                                            .as_ref()
                                            .to_file_path()
                                            .unwrap_or_else(|_| PathBuf::from(item.uri.path()))
                                    )
                                ))?
                            } else {
                                item.uri
                            };
                            let file = item_uri.as_ref().to_file_path().map_err(|_| {
                                KernelError::InvalidUri {
                                    message: item_uri.to_string(),
                                }
                            })?;
                            let source = fs::read(&file).map_err(|error| KernelError::Handler {
                                message: format!("read {}: {error}", file.display()),
                            })?;
                            matches.push(repository_anchored_text(
                                item_uri,
                                &file,
                                &source,
                                &self.structure,
                            )?);
                        }
                    }
                    Ok(OperationResult::Grep(Ok(matches)))
                }
                _ => Err(KernelError::UnsupportedVerb {
                    verb: "typed-repository".to_owned(),
                    uri: "repo://".to_owned(),
                }),
            }
        })
    }
}

impl RepositoryHandler {
    fn find_paths(&self, target: &ResourceAddress) -> Result<Vec<String>, KernelError> {
        let url = Self::url(target)?;
        if url.scheme() == "file" {
            let (file, _) = self.file_and_suffix(target)?;
            if file.is_dir() {
                return Ok(fff::find(&file, "")?
                    .into_iter()
                    .map(|path| path.display().to_string())
                    .collect());
            }
            return Ok(vec![file.display().to_string()]);
        }
        let prefix = url.path().trim_matches('/');
        let mut output = fff::find(&self.root, "")?
            .into_iter()
            .filter_map(|path| {
                let relative_path = relative(&self.root, &path);
                (prefix.is_empty() || relative_path.starts_with(prefix))
                    .then(|| format!("repo://{}/{}", self.project, relative_path))
            })
            .collect::<Vec<_>>();
        output.sort();
        Ok(output)
    }

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
        let mut matches = fff::grep(&root, pattern, &self.structure)?;
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
    let mut value = serde_json::to_value(declaration).unwrap_or_else(|_| json!({}));
    if let Value::Object(fields) = &mut value {
        let anchor = declaration_anchor(declaration.start_line, lines, anchors)
            .map(|anchor| format!("{file_uri}#{}", anchor));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kernel, ResourceUri};
    use tempfile::tempdir;

    #[tokio::test]
    async fn exposes_rust_symbols_as_repo_views() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("main.rs"), "fn answer() -> u8 { 42 }\n").unwrap();
        let kernel = Kernel::new();
        let handler = RepositoryHandler::new(root.path()).unwrap();
        let project = handler.project().to_owned();
        kernel.register(handler).await;
        let target = format!("repo://{project}/main.rs/symbols");
        let result = kernel
            .execute(Request::new(
                Verb::Read,
                ResourceUri::parse(&target).unwrap(),
                Value::Null,
            ))
            .await;
        assert!(result.ok);
        assert_eq!(result.value.unwrap()["items"][0]["name"], "answer");
    }

    #[tokio::test]
    async fn typed_repository_read_returns_anchored_source() {
        let root = tempdir().unwrap();
        let file = root.path().join("main.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let handler = RepositoryHandler::new(root.path()).unwrap();
        let uri = crate::ResourceUri::parse(&file.display().to_string()).unwrap();
        let kernel = Kernel::new();
        kernel.register_typed(handler).await;
        let result = kernel
            .execute_operation(crate::Operation::Read(vec![crate::ReadRequest {
                uri: uri.clone(),
                at: None,
                before: None,
                after: None,
            }]))
            .await
            .unwrap();
        let crate::OperationResult::Read(mut values) = result else {
            panic!("wrong typed repository result")
        };
        let Ok(crate::ReadResult::Text(text)) = values.remove(0) else {
            panic!("wrong typed repository value")
        };
        assert_eq!(text.uri, uri);
        assert_eq!(text.lines[0].text, "fn main() {}");
        assert!(!text.lines[0].anchor.tokens().is_empty());
    }

    #[tokio::test]
    async fn exposes_non_rust_symbols_through_the_vendored_ast_engine() {
        let root = tempdir().unwrap();
        fs::write(
            root.path().join("main.py"),
            "class Greeter:\n    def hello(self):\n        return 'hi'\n",
        )
        .unwrap();
        let kernel = Kernel::new();
        let handler = RepositoryHandler::new(root.path()).unwrap();
        let project = handler.project().to_owned();
        kernel.register(handler).await;
        let target = format!("repo://{project}/main.py/symbols");
        let result = kernel
            .execute(Request::new(
                Verb::Read,
                ResourceUri::parse(&target).unwrap(),
                Value::Null,
            ))
            .await;
        assert!(result.ok);
        let items = result.value.unwrap()["items"].as_array().unwrap().clone();
        assert!(items.iter().any(|item| item["name"] == "Greeter"));
        assert!(items.iter().any(|item| item["name"] == "hello"));
        assert!(
            items
                .iter()
                .all(|item| item["provider"] == "ast-bro:python")
        );
    }

    #[tokio::test]
    async fn exposes_structural_and_project_ast_views() {
        let root = tempdir().unwrap();
        fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            "pub trait Shape {}\npub struct Circle;\nimpl Shape for Circle {}\npub fn draw() { paint(); }\npub fn paint() {}\n",
        )
        .unwrap();
        let kernel = Kernel::new();
        let handler = RepositoryHandler::new(root.path()).unwrap();
        let project = handler.project().to_owned();
        kernel.register(handler).await;
        for suffix in [
            "src/lib.rs/map",
            "src/lib.rs/show/Circle",
            "deps",
            "surface",
        ] {
            let target = format!("repo://{project}/{suffix}");
            let result = kernel
                .execute(Request::new(
                    Verb::Read,
                    ResourceUri::parse(&target).unwrap(),
                    Value::Null,
                ))
                .await;
            assert!(result.ok, "AST view failed: {target}: {:?}", result.error);
        }
        for suffix in [
            "src/lib.rs/symbols/draw/callees",
            "src/lib.rs/symbols/draw/trace/to/paint",
            "src/lib.rs/symbols/Shape/implementations",
            "symbols/Circle",
        ] {
            let target = format!("repo://{project}/{suffix}");
            let result = kernel
                .execute(Request::new(
                    Verb::Read,
                    ResourceUri::parse(&target).unwrap(),
                    Value::Null,
                ))
                .await;
            assert!(
                result.ok,
                "relationship URI failed: {target}: {:?}",
                result.error
            );
        }
    }

    #[tokio::test]
    async fn exposes_the_same_ast_paths_for_file_uris() {
        let root = tempdir().unwrap();
        let file = root.path().join("main.py");
        fs::write(&file, "def hello():\n    return 1\n").unwrap();
        let kernel = Kernel::new();
        kernel
            .register(RepositoryHandler::new(root.path()).unwrap())
            .await;
        let file_uri = Url::from_file_path(&file).unwrap().to_string();
        let target = format!("{file_uri}/symbols");
        let result = kernel
            .execute(Request::new(
                Verb::Read,
                ResourceUri::parse(&target).unwrap(),
                Value::Null,
            ))
            .await;
        assert!(result.ok, "local AST view failed: {:?}", result.error);
        let value = result.value.unwrap();
        assert_eq!(value["items"][0]["name"], "hello");
        assert!(
            value["items"][0]["def"]
                .as_str()
                .is_some_and(|def| def.starts_with("file://"))
        );

        let bare_target = format!("{}/symbols", file.display());
        let result = kernel
            .execute(Request::new(
                Verb::Read,
                ResourceAddress::path(bare_target),
                Value::Null,
            ))
            .await;
        assert!(result.ok, "bare local AST view failed: {:?}", result.error);
        assert_eq!(result.value.unwrap()["items"][0]["name"], "hello");

        let result = kernel
            .execute(Request::new(
                Verb::Find,
                ResourceAddress::path(format!("{}/symbols", file.display())),
                Value::Null,
            ))
            .await;
        assert!(result.ok, "bare local find failed: {:?}", result.error);

        let result = kernel
            .execute(Request::new(
                Verb::Grep,
                ResourceAddress::path(format!("{}/symbols", file.display())),
                json!({"pattern":"hello"}),
            ))
            .await;
        assert!(result.ok, "bare local grep failed: {:?}", result.error);
    }
}
