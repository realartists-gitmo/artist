use crate::{
    Anchor, AnchorError, AnchorSet, AnchoredLine, AnchoredText, BoxFuture, Handler,
    HandlerDescriptor, KernelError, KernelHandle, Operation, OperationResult, Pattern, ReadResult,
    Request, ResourceAddress, SearchService, StructuralAnalyzer, StructuralLine, TypedHandler,
    Verb, address::uri_path,
};
use cap_std::{ambient_authority, fs::Dir};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Native filesystem handler constrained to one root directory.
pub struct FileHandler {
    root: PathBuf,
    /// Capability-scoped access to the semantic project root.  `root` remains
    /// useful for URI/debugging, but filesystem reads and directory mutations
    /// in the typed path are performed relative to this handle.
    dir: Dir,
    structure: StructuralAnalyzer,
    search: SearchService,
}

impl FileHandler {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, KernelError> {
        let root_path = root.as_ref();
        let root = fs::canonicalize(root_path).map_err(|error| KernelError::Handler {
            message: format!(
                "canonicalize filesystem root {}: {error}",
                root_path.display()
            ),
        })?;
        if !root.is_dir() {
            return Err(KernelError::Handler {
                message: format!("filesystem root is not a directory: {}", root.display()),
            });
        }
        let dir = Dir::open_ambient_dir(&root, ambient_authority()).map_err(|error| {
            KernelError::Handler {
                message: format!("open filesystem capability {}: {error}", root.display()),
            }
        })?;
        Ok(Self {
            root,
            dir,
            structure: StructuralAnalyzer::default(),
            search: SearchService::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Analyze a file using CST/type-kind context when available, degrading to
    /// line-only identity when no usable parser exists.
    pub fn structural_lines(&self, path: &Path, bytes: &[u8]) -> (String, Vec<StructuralLine>) {
        self.structure.analyze(path, bytes)
    }

    fn is_virtual_projection(&self, uri: &crate::ResourceUri) -> bool {
        let Ok(path) = uri.as_ref().to_file_path() else {
            return false;
        };
        let Ok(relative) = self.requested_relative(&path) else {
            return false;
        };
        let mut current = self.root.join(relative);
        if current.is_file() {
            return false;
        }
        while let Some(parent) = current.parent() {
            if parent.is_file() {
                return true;
            }
            if parent == self.root {
                break;
            }
            current = parent.to_owned();
        }
        false
    }

    fn typed_path(&self, uri: &crate::ResourceUri) -> Result<PathBuf, KernelError> {
        let path = self.typed_path_syntax(uri)?;
        self.resolve_existing(&path)
    }

    /// Resolve only address and containment. Claiming a URI must not probe
    /// the filesystem: an absent file is still owned by this namespace and
    /// should produce NotFound during execution.
    fn typed_path_syntax(&self, uri: &crate::ResourceUri) -> Result<PathBuf, KernelError> {
        if uri.scheme() != "file" {
            return Err(KernelError::UnsupportedUri {
                uri: uri.to_string(),
            });
        }
        let path = uri
            .as_ref()
            .to_file_path()
            .map_err(|_| KernelError::InvalidUri {
                message: uri.to_string(),
            })?;
        let relative = self.requested_relative(&path)?;
        let resolved = self.root.join(relative);
        self.ensure_in_root(&resolved)?;
        Ok(resolved)
    }

    fn anchored_text(
        &self,
        uri: crate::ResourceUri,
        path: &Path,
        bytes: &[u8],
    ) -> Result<AnchoredText, KernelError> {
        let (_, lines) = self.structural_lines(path, bytes);
        let inputs = lines
            .iter()
            .map(StructuralLine::anchor_input)
            .collect::<Vec<_>>();
        let anchors = if inputs.is_empty() {
            None
        } else {
            Some(AnchorSet::from_inputs(&inputs).map_err(anchor_error)?)
        };
        let output = lines
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                let ending = match bytes.get(line.end_byte..) {
                    Some([b'\r', b'\n', ..]) => crate::LineEnding::Crlf,
                    Some([b'\n', ..]) => crate::LineEnding::Lf,
                    Some([b'\r', ..]) => crate::LineEnding::Cr,
                    _ => crate::LineEnding::None,
                };
                let anchor = anchors
                    .as_ref()
                    .and_then(|set| set.items().get(index))
                    .map(|item| item.anchor.clone())
                    .unwrap_or_else(|| Anchor::from_tokens(vec![(index + 1).to_string()]));
                Ok(AnchoredLine {
                    anchor,
                    text: String::from_utf8(line.line_text).map_err(|error| {
                        KernelError::Handler {
                            message: error.to_string(),
                        }
                    })?,
                    ending,
                })
            })
            .collect::<Result<Vec<_>, KernelError>>()?;
        Ok(AnchoredText { uri, lines: output })
    }

    fn resolve_existing(&self, requested: &Path) -> Result<PathBuf, KernelError> {
        let relative = self.requested_relative(requested)?;
        let resolved = self.dir.canonicalize(&relative).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                KernelError::NotFound {
                    uri: requested.display().to_string(),
                }
            } else {
                KernelError::Handler {
                    message: format!("resolve {}: {error}", requested.display()),
                }
            }
        })?;
        let resolved = self.root.join(resolved);
        self.ensure_in_root(&resolved)?;
        Ok(resolved)
    }

    fn requested_relative(&self, requested: &Path) -> Result<PathBuf, KernelError> {
        if requested.is_absolute() {
            let relative = requested
                .strip_prefix(&self.root)
                .map(Path::to_owned)
                .map_err(|_| KernelError::InvalidRequest {
                    message: format!("path escapes filesystem root: {}", requested.display()),
                })?;
            Ok(if relative.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                relative
            })
        } else {
            Ok(requested.to_owned())
        }
    }

    fn relative_path(&self, path: &Path) -> Result<PathBuf, KernelError> {
        path.strip_prefix(&self.root)
            .map(Path::to_owned)
            .map_err(|_| KernelError::InvalidRequest {
                message: format!("path escapes filesystem root: {}", path.display()),
            })
    }

    fn cap_read(&self, path: &Path) -> Result<Vec<u8>, KernelError> {
        let relative = self.relative_path(path)?;
        self.dir
            .read(&relative)
            .map_err(|error| KernelError::Handler {
                message: format!("read {}: {error}", path.display()),
            })
    }

    fn cap_metadata(&self, path: &Path) -> Result<cap_std::fs::Metadata, KernelError> {
        let relative = self.relative_path(path)?;
        self.dir
            .metadata(&relative)
            .map_err(|error| KernelError::Handler {
                message: format!("stat {}: {error}", path.display()),
            })
    }

    fn cap_read_dir(&self, path: &Path) -> Result<cap_std::fs::ReadDir, KernelError> {
        let relative = self.relative_path(path)?;
        self.dir
            .read_dir(&relative)
            .map_err(|error| KernelError::Handler {
                message: format!("read directory {}: {error}", path.display()),
            })
    }

    fn cap_atomic_replace(&self, path: &Path, bytes: &[u8]) -> Result<(), KernelError> {
        let relative = self.relative_path(path)?;
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        if !parent.as_os_str().is_empty() {
            self.dir
                .create_dir_all(parent)
                .map_err(|error| KernelError::Handler {
                    message: format!("create parent for {}: {error}", path.display()),
                })?;
        }
        let name = path
            .file_name()
            .ok_or_else(|| KernelError::InvalidRequest {
                message: format!("path has no filename: {}", path.display()),
            })?
            .to_string_lossy();
        let temporary = parent.join(format!(
            ".{name}.artist-tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file =
            self.dir
                .open_with(&temporary, &options)
                .map_err(|error| KernelError::Handler {
                    message: format!("create temporary replacement: {error}"),
                })?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| KernelError::Handler {
                message: format!("write temporary replacement: {error}"),
            })?;
        self.dir
            .rename(&temporary, &self.dir, &relative)
            .map_err(|error| KernelError::Handler {
                message: format!("atomically replace {}: {error}", path.display()),
            })
    }

    fn cap_remove_file(&self, path: &Path) -> Result<(), KernelError> {
        let relative = self.relative_path(path)?;
        self.dir
            .remove_file(&relative)
            .map_err(|error| KernelError::Handler {
                message: format!("delete {}: {error}", path.display()),
            })
    }

    fn cap_remove_dir(&self, path: &Path) -> Result<(), KernelError> {
        let relative = self.relative_path(path)?;
        self.dir
            .remove_dir(&relative)
            .map_err(|error| KernelError::Handler {
                message: format!("delete {}: {error}", path.display()),
            })
    }

    fn resolve_for_write(&self, requested: &Path) -> Result<PathBuf, KernelError> {
        let relative = self.requested_relative(requested)?;
        let parent = relative
            .parent()
            .ok_or_else(|| KernelError::InvalidRequest {
                message: format!("path has no parent: {}", requested.display()),
            })?;
        let parent_to_resolve = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let parent =
            self.dir
                .canonicalize(parent_to_resolve)
                .map_err(|error| KernelError::Handler {
                    message: format!("resolve parent {}: {error}", parent_to_resolve.display()),
                })?;
        let parent = self.root.join(parent);
        self.ensure_in_root(&parent)?;
        Ok(parent.join(
            relative
                .file_name()
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: format!("path has no filename: {}", requested.display()),
                })?,
        ))
    }

    fn ensure_in_root(&self, path: &Path) -> Result<(), KernelError> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(KernelError::InvalidRequest {
                message: format!("path escapes filesystem root: {}", path.display()),
            })
        }
    }

    fn read_value(&self, path: &Path) -> Result<Value, KernelError> {
        let metadata = self.cap_metadata(path)?;
        if metadata.is_dir() {
            let mut entries = self
                .cap_read_dir(path)?
                .map(|entry| {
                    let entry = entry.map_err(|error| KernelError::Handler {
                        message: format!("read directory entry: {error}"),
                    })?;
                    let file_type = entry.file_type().map_err(|error| KernelError::Handler {
                        message: format!("read entry type: {error}"),
                    })?;
                    Ok(json!({
                        "name": entry.file_name().to_string_lossy(),
                        "path": path.join(entry.file_name()).to_string_lossy(),
                        "directory": file_type.is_dir(),
                    }))
                })
                .collect::<Result<Vec<_>, KernelError>>()?;
            entries.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
            return Ok(json!({"type": "directory", "entries": entries}));
        }

        let bytes = self.cap_read(path)?;
        match String::from_utf8(bytes.clone()) {
            Ok(content) => Ok(json!({"type": "text", "content": content})),
            Err(_) => Ok(json!({
                "type": "binary",
                "bytes": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    bytes,
                ),
            })),
        }
    }

    fn edit(&self, requested: &Path, args: &Value) -> Result<Value, KernelError> {
        let path = self.resolve_existing(requested)?;
        let original = self.cap_read(&path)?;
        let (provider, lines) = self.structural_lines(&path, &original);
        let inputs = lines
            .iter()
            .map(StructuralLine::anchor_input)
            .collect::<Vec<_>>();
        let anchors = AnchorSet::from_inputs(&inputs).map_err(anchor_error)?;
        let edits: Vec<EditSpec> =
            serde_json::from_value(args.get("edits").cloned().ok_or_else(|| {
                KernelError::InvalidRequest {
                    message: "filesystem edit requires an args.edits array".to_owned(),
                }
            })?)
            .map_err(|error| KernelError::InvalidRequest {
                message: format!("invalid filesystem edit: {error}"),
            })?;
        if edits.is_empty() {
            return Err(KernelError::InvalidRequest {
                message: "filesystem edit requires at least one edit".to_owned(),
            });
        }

        let mut ranges = Vec::with_capacity(edits.len());
        for edit in edits {
            let start = anchors.resolve(&edit.start).map_err(anchor_error)?;
            let end = edit
                .end
                .as_ref()
                .map(|anchor| anchors.resolve(anchor))
                .transpose()
                .map_err(anchor_error)?
                .unwrap_or(start);
            if start > end {
                return Err(KernelError::InvalidAnchor {
                    message: format!("edit range is reversed: {start}..={end}"),
                });
            }
            ranges.push((
                lines[start].start_byte,
                lines[end].end_byte,
                edit.replacement.into_bytes(),
            ));
        }
        ranges.sort_by_key(|(start, _, _)| *start);
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(KernelError::InvalidRequest {
                message: "filesystem edit ranges overlap".to_owned(),
            });
        }

        let mut rendered = Vec::with_capacity(original.len());
        let mut cursor = 0;
        for (start, end, replacement) in ranges {
            rendered.extend_from_slice(&original[cursor..start]);
            rendered.extend_from_slice(&replacement);
            cursor = end;
        }
        rendered.extend_from_slice(&original[cursor..]);
        self.cap_atomic_replace(&path, &rendered)?;
        Ok(json!({
            "edited": true,
            "path": path,
            "provider": provider,
        }))
    }
}

#[derive(Debug, Deserialize)]
struct EditSpec {
    start: Anchor,
    #[serde(default)]
    end: Option<Anchor>,
    replacement: String,
}

fn anchor_error(error: AnchorError) -> KernelError {
    KernelError::InvalidAnchor {
        message: error.to_string(),
    }
}

fn select_read_window(
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
        _ => (index.saturating_sub(before), (index + after + 1).min(count)),
    };
    text.lines = text.lines.drain(start..end).collect();
    Ok(text)
}

impl FileHandler {
    fn apply_typed_edit(
        &self,
        path: &Path,
        uri: crate::ResourceUri,
        operations: &[crate::EditOperation],
    ) -> Result<crate::EditResult, KernelError> {
        let original = self.cap_read(path).map_err(|error| KernelError::Handler {
            message: format!("read {}: {error}", path.display()),
        })?;
        let (_, lines) = self.structural_lines(path, &original);
        let inputs = lines
            .iter()
            .map(StructuralLine::anchor_input)
            .collect::<Vec<_>>();
        let anchors = AnchorSet::from_inputs(&inputs).map_err(anchor_error)?;
        let old = self.anchored_text(uri.clone(), path, &original)?;
        let mut ranges = Vec::with_capacity(operations.len());
        for operation in operations {
            match operation {
                crate::EditOperation::Replace(replace) => {
                    let start = anchors.resolve(&replace.start).map_err(anchor_error)?;
                    let end = replace
                        .end
                        .as_ref()
                        .map(|anchor| anchors.resolve(anchor))
                        .transpose()
                        .map_err(anchor_error)?
                        .unwrap_or(start);
                    if start > end {
                        return Err(KernelError::InvalidAnchor {
                            message: format!("edit range is reversed: {start}..={end}"),
                        });
                    }
                    ranges.push((
                        lines[start].start_byte,
                        lines[end].end_byte,
                        replace.content.as_bytes().to_vec(),
                    ));
                }
                crate::EditOperation::Insert(insert) => {
                    let offset = match &insert.at {
                        crate::InsertionPoint::Top => 0,
                        crate::InsertionPoint::Bottom => original.len(),
                        crate::InsertionPoint::Before(anchor) => {
                            let index = anchors.resolve(anchor).map_err(anchor_error)?;
                            lines[index].start_byte
                        }
                        crate::InsertionPoint::After(anchor) => {
                            let index = anchors.resolve(anchor).map_err(anchor_error)?;
                            let line = &lines[index];
                            let terminator = match original.get(line.end_byte..) {
                                Some([b'\r', b'\n', ..]) => 2,
                                Some([b'\r' | b'\n', ..]) => 1,
                                _ => 0,
                            };
                            line.end_byte + terminator
                        }
                    };
                    ranges.push((offset, offset, insert.content.as_bytes().to_vec()));
                }
            }
        }
        ranges.sort_by_key(|(start, end, _)| (*start, *end));
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(KernelError::InvalidRequest {
                message: "filesystem edit ranges overlap".to_owned(),
            });
        }
        let mut rendered = Vec::with_capacity(original.len());
        let mut cursor = 0;
        for (start, end, replacement) in ranges {
            rendered.extend_from_slice(&original[cursor..start]);
            rendered.extend_from_slice(&replacement);
            cursor = end;
        }
        rendered.extend_from_slice(&original[cursor..]);
        self.cap_atomic_replace(path, &rendered)?;
        let new = self.anchored_text(uri.clone(), path, &rendered)?;
        Ok(crate::EditResult {
            text: new.clone(),
            diff: crate::AnchoredDiff {
                uri,
                hunks: vec![crate::DiffHunk {
                    old: old.lines,
                    new: new.lines.clone(),
                }],
            },
        })
    }
}

impl Handler for FileHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            name: "filesystem".to_owned(),
            schemes: Vec::new(),
            verbs: vec![
                Verb::Read,
                Verb::Write,
                Verb::Edit,
                Verb::Delete,
                Verb::Find,
                Verb::Grep,
            ],
        }
    }

    fn claims(&self, address: &ResourceAddress) -> bool {
        address.as_uri().is_some_and(|uri| {
            uri.scheme() == "file" && uri.query().is_none() && !self.is_virtual_projection(uri)
        })
    }

    fn execute<'a>(
        &'a self,
        request: Request,
        _host: KernelHandle,
    ) -> BoxFuture<'a, Result<Value, KernelError>> {
        Box::pin(async move {
            let requested =
                uri_path(
                    request
                        .target
                        .as_uri()
                        .ok_or_else(|| KernelError::InvalidUri {
                            message: request.target.to_string(),
                        })?,
                )?;
            match request.verb {
                Verb::Read => self
                    .read_value(&self.resolve_existing(&requested)?)
                    .map(|value| json!({"path": requested, "value": value})),
                Verb::Write => {
                    let path = self.resolve_for_write(&requested)?;
                    let content = request
                        .args
                        .get("value")
                        .and_then(Value::as_str)
                        .ok_or_else(|| KernelError::InvalidRequest {
                            message: "filesystem write requires an args.value string".to_owned(),
                        })?;
                    self.cap_atomic_replace(&path, content.as_bytes())?;
                    Ok(json!({"written": true, "path": path}))
                }
                Verb::Edit => self.edit(&requested, &request.args),
                Verb::Delete => {
                    let path = self.resolve_existing(&requested)?;
                    if self.cap_metadata(&path)?.is_dir() {
                        if self.cap_read_dir(&path)?.next().is_some() {
                            return Err(KernelError::NotEmpty {
                                uri: request.target.to_string(),
                            });
                        }
                        self.cap_remove_dir(&path)
                    } else {
                        self.cap_remove_file(&path)
                    }
                    .map_err(|error| KernelError::Handler {
                        message: format!("delete {}: {error}", path.display()),
                    })?;
                    Ok(json!({"deleted": true, "path": path}))
                }
                Verb::Find => {
                    let path = self.resolve_existing(&requested)?;
                    let pattern = Pattern::parse(
                        request
                            .args
                            .get("query")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )?;
                    let files = self.search.find_files(&path, &pattern)?;
                    Ok(json!({"paths": files}))
                }
                Verb::Grep => {
                    let path = self.resolve_existing(&requested)?;
                    let pattern = request
                        .args
                        .get("pattern")
                        .and_then(Value::as_str)
                        .ok_or_else(|| KernelError::InvalidRequest {
                            message: "filesystem grep requires an args.pattern string".to_owned(),
                        })?;
                    let matches =
                        self.search
                            .grep_file(&path, &Pattern::parse(pattern)?, &self.structure)?;
                    let paths = matches
                        .iter()
                        .map(|item| item.uri.to_string())
                        .collect::<Vec<_>>();
                    Ok(json!({"matches": matches, "paths": paths, "pattern": pattern}))
                }
                verb => Err(KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: request.target.to_string(),
                }),
            }
        })
    }
}

impl TypedHandler for FileHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            name: "filesystem-typed".to_owned(),
            schemes: vec!["file".to_owned()],
            verbs: vec![
                Verb::Read,
                Verb::Write,
                Verb::Edit,
                Verb::Delete,
                Verb::Find,
                Verb::Grep,
            ],
        }
    }

    fn claims_operation(&self, operation: &Operation) -> bool {
        let ordinary = |uri: &crate::ResourceUri| {
            uri.scheme() == "file" && uri.query().is_none() && !self.is_virtual_projection(uri)
        };
        match operation {
            Operation::Read(requests) => requests.iter().all(|request| {
                ordinary(&request.uri) && self.typed_path_syntax(&request.uri).is_ok()
            }),
            Operation::Write(requests) => requests.iter().all(|request| {
                ordinary(&request.uri) && request.uri.as_ref().to_file_path().is_ok()
            }),
            Operation::Edit(requests) => requests.iter().all(|request| {
                ordinary(&request.uri) && request.uri.as_ref().to_file_path().is_ok()
            }),
            Operation::Delete(uris) => uris
                .iter()
                .all(|uri| ordinary(uri) && self.typed_path_syntax(uri).is_ok()),
            Operation::Find(request) => {
                !request.roots.is_empty()
                    && request
                        .roots
                        .iter()
                        .all(|uri| ordinary(uri) && self.typed_path_syntax(uri).is_ok())
            }
            Operation::Grep(request) => {
                matches!(&request.source,
                crate::GrepSource::Resources(uris) if !uris.is_empty() && uris.iter().all(|uri| ordinary(uri) && self.typed_path_syntax(uri).is_ok()))
                    || matches!(&request.source, crate::GrepSource::Text(text) if !text.is_empty())
            }
            _ => false,
        }
    }

    fn execute_typed<'a>(
        &'a self,
        operation: Operation,
        _host: KernelHandle,
        _context: crate::InvocationContext,
    ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
        Box::pin(async move {
            match operation {
                Operation::Read(requests) => {
                    let results = requests
                        .into_iter()
                        .map(|request| {
                            let path = self.typed_path(&request.uri)?;
                            if self.cap_metadata(&path)?.is_dir() {
                                let mut entries = self
                                    .cap_read_dir(&path)?
                                    .map(|entry| {
                                        let entry =
                                            entry.map_err(|error| KernelError::Handler {
                                                message: error.to_string(),
                                            })?;
                                        let path = path.join(entry.file_name());
                                        let is_dir = entry
                                            .file_type()
                                            .map_err(|error| KernelError::Handler {
                                                message: error.to_string(),
                                            })?
                                            .is_dir();
                                        crate::ResourceUri::parse(&format!(
                                            "{}{}",
                                            path.display(),
                                            if is_dir { "/" } else { "" }
                                        ))
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                entries.sort_by_key(ToString::to_string);
                                Ok(ReadResult::Directory {
                                    uri: request.uri,
                                    entries,
                                })
                            } else {
                                let bytes = self.cap_read(&path)?;
                                let text = self.anchored_text(request.uri, &path, &bytes)?;
                                Ok(ReadResult::Text(select_read_window(
                                    text,
                                    request.at.as_ref(),
                                    request.before,
                                    request.after,
                                )?))
                            }
                        })
                        .collect();
                    Ok(OperationResult::Read(results))
                }
                Operation::Write(requests) => {
                    let results = requests
                        .into_iter()
                        .map(|request| {
                            let requested = request.uri.as_ref().to_file_path().map_err(|_| {
                                KernelError::InvalidUri {
                                    message: request.uri.to_string(),
                                }
                            })?;
                            let path = self.resolve_for_write(&requested)?;
                            self.cap_atomic_replace(&path, request.content.as_bytes())?;
                            let bytes = self.cap_read(&path)?;
                            Ok(crate::WriteResult {
                                text: self.anchored_text(request.uri, &path, &bytes)?,
                            })
                        })
                        .collect();
                    Ok(OperationResult::Write(results))
                }
                Operation::Edit(requests) => {
                    let results = requests
                        .into_iter()
                        .map(|request| {
                            let path = self.typed_path(&request.uri)?;
                            self.apply_typed_edit(&path, request.uri, &request.operations)
                        })
                        .collect();
                    Ok(OperationResult::Edit(results))
                }
                Operation::Delete(uris) => {
                    let results = uris
                        .into_iter()
                        .map(|uri| {
                            let path = self.typed_path(&uri)?;
                            if self.cap_metadata(&path)?.is_dir() {
                                if self.cap_read_dir(&path)?.next().is_some() {
                                    return Err(KernelError::NotEmpty {
                                        uri: uri.to_string(),
                                    });
                                }
                                self.cap_remove_dir(&path)
                            } else {
                                self.cap_remove_file(&path)
                            }
                            .map_err(|error| KernelError::Handler {
                                message: error.to_string(),
                            })?;
                            Ok(uri)
                        })
                        .collect();
                    Ok(OperationResult::Delete(results))
                }
                Operation::Find(request) => {
                    let mut paths = Vec::new();
                    for root in request.roots {
                        paths.extend(self.search.find_files(
                            &self.typed_path(&root)?,
                            &Pattern::parse(&request.query)?,
                        )?);
                    }
                    let mut seen = std::collections::HashSet::new();
                    paths.retain(|path| seen.insert(path.clone()));
                    Ok(OperationResult::Find(
                        paths
                            .into_iter()
                            .map(|path| {
                                crate::ResourceUri::parse(&format!(
                                    "{}{}",
                                    path.display(),
                                    if path.is_dir() { "/" } else { "" }
                                ))
                            })
                            .collect(),
                    ))
                }
                Operation::Grep(request) => {
                    let pattern = Pattern::parse(&request.pattern)?;
                    let matches = match request.source {
                        crate::GrepSource::Resources(uris) => {
                            let mut matches = Vec::new();
                            for uri in uris {
                                matches.extend(self.search.grep_file(
                                    &self.typed_path(&uri)?,
                                    &pattern,
                                    &self.structure,
                                )?);
                            }
                            matches
                        }
                        crate::GrepSource::Text(text) => SearchService::grep_text(&text, &pattern)?,
                    };
                    Ok(OperationResult::Grep(Ok(matches)))
                }
                _ => unreachable!(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kernel, ResourceAddress};
    use tempfile::tempdir;

    fn request(verb: Verb, path: &Path, args: Value) -> Request {
        Request::new(verb, ResourceAddress::path(path), args)
    }

    #[tokio::test]
    async fn reads_writes_finds_greps_and_deletes_native_files() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let kernel = Kernel::new();
        kernel
            .register(FileHandler::new(root.path()).unwrap())
            .await;

        let path = root.path().join("src/main.rs");
        let read = kernel
            .execute(request(Verb::Read, &path, Value::Null))
            .await;
        assert_eq!(read.value.as_ref().unwrap()["value"]["type"], "text");
        assert_eq!(
            read.value.as_ref().unwrap()["value"]["content"],
            "fn main() {}\n"
        );

        let new_path = root.path().join("src/lib.rs");
        assert!(
            kernel
                .execute(request(
                    Verb::Write,
                    &new_path,
                    json!({"value": "pub fn answer() -> u8 { 42 }"}),
                ))
                .await
                .ok
        );
        let found = kernel
            .execute(request(Verb::Find, root.path(), json!({"query": "rs"})))
            .await;
        assert_eq!(
            found.value.as_ref().unwrap()["paths"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let matches = kernel
            .execute(request(
                Verb::Grep,
                root.path(),
                json!({"pattern": "answer"}),
            ))
            .await;
        assert_eq!(
            matches.value.as_ref().unwrap()["paths"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        assert!(
            kernel
                .execute(request(Verb::Delete, &new_path, Value::Null))
                .await
                .ok
        );
        assert!(
            !kernel
                .execute(request(Verb::Read, &new_path, Value::Null))
                .await
                .ok
        );
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_filesystem_root() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let path = outside.path().join("secret.txt");
        fs::write(&path, "secret").unwrap();
        let kernel = Kernel::new();
        kernel
            .register(FileHandler::new(root.path()).unwrap())
            .await;

        let result = kernel
            .execute(request(Verb::Read, &path, Value::Null))
            .await;
        assert!(!result.ok);
        assert!(matches!(
            result.error,
            Some(KernelError::InvalidRequest { .. })
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capability_root_rejects_symlink_escape() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join("link.txt"),
        )
        .unwrap();
        let kernel = Kernel::new();
        kernel
            .register(FileHandler::new(root.path()).unwrap())
            .await;

        let result = kernel
            .execute(request(
                Verb::Read,
                &root.path().join("link.txt"),
                Value::Null,
            ))
            .await;
        assert!(!result.ok);
        assert!(matches!(
            result.error,
            Some(KernelError::InvalidRequest { .. }) | Some(KernelError::Handler { .. })
        ));
    }

    #[tokio::test]
    async fn edits_cst_addressed_lines_atomically() {
        let root = tempdir().unwrap();
        let path = root.path().join("main.rs");
        fs::write(&path, "fn main() {\n    let answer = 41;\n}\n").unwrap();
        let handler = FileHandler::new(root.path()).unwrap();
        let bytes = fs::read(&path).unwrap();
        let (_, lines) = handler.structural_lines(&path, &bytes);
        let anchors = AnchorSet::from_inputs(
            &lines
                .iter()
                .map(StructuralLine::anchor_input)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let kernel = Kernel::new();
        kernel.register(handler).await;
        let result = kernel
            .execute(request(
                Verb::Edit,
                &path,
                json!({"edits": [{"start": anchors.items()[1].anchor, "replacement": "    let answer = 42;"}]}),
            ))
            .await;
        assert!(result.ok);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "fn main() {\n    let answer = 42;\n}\n"
        );
    }

    #[tokio::test]
    async fn rejects_stale_or_overlapping_edits_without_writing() {
        let root = tempdir().unwrap();
        let path = root.path().join("main.rs");
        let original = "fn main() {\n    let answer = 41;\n}\n";
        fs::write(&path, original).unwrap();
        let handler = FileHandler::new(root.path()).unwrap();
        let (_, lines) = handler.structural_lines(&path, original.as_bytes());
        let anchors = AnchorSet::from_inputs(
            &lines
                .iter()
                .map(StructuralLine::anchor_input)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let kernel = Kernel::new();
        kernel.register(handler).await;

        let stale = Anchor::from_tokens(vec!["stale".to_owned()]);
        let result = kernel
            .execute(request(
                Verb::Edit,
                &path,
                json!({"edits": [{"start": stale, "replacement": "changed"}]}),
            ))
            .await;
        assert!(matches!(
            result.error,
            Some(KernelError::InvalidAnchor { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);

        let result = kernel
            .execute(request(
                Verb::Edit,
                &path,
                json!({"edits": [{"start": anchors.items()[0].anchor, "end": anchors.items()[1].anchor, "replacement": "x"}, {"start": anchors.items()[1].anchor, "replacement": "y"}]}),
            ))
            .await;
        assert!(matches!(
            result.error,
            Some(KernelError::InvalidRequest { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn exposes_cst_context_for_rust_and_line_fallback_for_unknown_files() {
        let root = tempdir().unwrap();
        let handler = FileHandler::new(root.path()).unwrap();
        let (provider, rust_lines) = handler.structural_lines(
            Path::new("main.rs"),
            b"fn main() {\n    let answer = 42;\n}\n",
        );
        assert_eq!(provider, "tree-sitter-rust");
        assert!(
            rust_lines[1]
                .type_kind_chain
                .iter()
                .any(|kind| kind == "let_declaration")
        );

        let (provider, text_lines) =
            handler.structural_lines(Path::new("notes.txt"), b"one\ntwo\n");
        assert_eq!(provider, "line-fallback");
        assert!(
            text_lines
                .iter()
                .all(|line| line.type_kind_chain.is_empty())
        );
    }

    #[tokio::test]
    async fn typed_read_and_grep_use_contract_values_and_fff() {
        let root = tempdir().unwrap();
        let path = root.path().join("main.rs");
        fs::write(&path, "fn main() {\n    let answer = 42;\n}\n").unwrap();
        let handler = FileHandler::new(root.path()).unwrap();
        let uri = crate::ResourceUri::parse(&path.display().to_string()).unwrap();
        let kernel = Kernel::new();
        kernel.register_typed(handler).await;

        let read = kernel
            .execute_operation(crate::Operation::Read(vec![crate::ReadRequest {
                uri: uri.clone(),
                at: None,
                before: None,
                after: None,
            }]))
            .await
            .unwrap();
        let crate::OperationResult::Read(results) = read else {
            panic!("wrong typed result")
        };
        let Ok(crate::ReadResult::Text(text)) = &results[0] else {
            panic!("wrong read value")
        };
        assert_eq!(text.lines.len(), 3);
        assert!(!text.lines[1].anchor.tokens().is_empty());

        let grep = kernel
            .execute_operation(crate::Operation::Grep(crate::GrepRequest {
                pattern: "answer".to_owned(),
                source: crate::GrepSource::Resources(vec![uri.clone()]),
            }))
            .await
            .unwrap();
        let crate::OperationResult::Grep(Ok(matches)) = grep else {
            panic!("wrong grep result")
        };
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].lines[0].text, "    let answer = 42;");

        let grep_text = kernel
            .execute_operation(crate::Operation::Grep(crate::GrepRequest {
                pattern: "answer".to_owned(),
                source: crate::GrepSource::Text(vec![text.clone()]),
            }))
            .await
            .unwrap();
        let crate::OperationResult::Grep(Ok(text_matches)) = grep_text else {
            panic!("wrong text grep result")
        };
        assert_eq!(text_matches[0].lines[0].anchor, text.lines[1].anchor);

        let edit = kernel
            .execute_operation(crate::Operation::Edit(vec![crate::EditRequest {
                uri,
                operations: vec![crate::EditOperation::Replace(crate::ReplaceOperation {
                    start: text_matches[0].lines[0].anchor.clone(),
                    end: None,
                    content: "    let answer = 43;".to_owned(),
                })],
            }]))
            .await
            .unwrap();
        assert!(matches!(edit, crate::OperationResult::Edit(_)));
        assert!(fs::read_to_string(&path).unwrap().contains("answer = 43"));
    }

    #[tokio::test]
    async fn typed_edit_returns_diff_and_supports_insertions() {
        let root = tempdir().unwrap();
        let path = root.path().join("notes.txt");
        fs::write(&path, "one\ntwo\n").unwrap();
        let handler = FileHandler::new(root.path()).unwrap();
        let uri = crate::ResourceUri::parse(&path.display().to_string()).unwrap();
        let (_, lines) = handler.structural_lines(&path, b"one\ntwo\n");
        let anchors = AnchorSet::from_inputs(
            &lines
                .iter()
                .map(StructuralLine::anchor_input)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let kernel = Kernel::new();
        kernel.register_typed(handler).await;
        let result = kernel
            .execute_operation(crate::Operation::Edit(vec![crate::EditRequest {
                uri: uri.clone(),
                operations: vec![crate::EditOperation::Insert(crate::InsertOperation {
                    at: crate::InsertionPoint::Before(anchors.items()[1].anchor.clone()),
                    content: "inserted\n".to_owned(),
                })],
            }]))
            .await
            .unwrap();
        let crate::OperationResult::Edit(mut values) = result else {
            panic!("wrong typed result")
        };
        let value = values.remove(0).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\ninserted\ntwo\n");
        assert_eq!(value.text.uri, uri);
        assert_eq!(value.diff.hunks.len(), 1);
        assert_eq!(value.text.lines.len(), 3);
    }

    #[tokio::test]
    async fn typed_read_honors_windows_and_all_line_endings() {
        let root = tempdir().unwrap();
        let path = root.path().join("mixed.txt");
        fs::write(&path, b"one\r\ntwo\rthree\nfour").unwrap();
        let uri = crate::ResourceUri::parse(&path.display().to_string()).unwrap();
        let kernel = Kernel::new();
        kernel
            .register_typed(FileHandler::new(root.path()).unwrap())
            .await;
        let full = kernel
            .execute_operation(crate::Operation::Read(vec![crate::ReadRequest {
                uri: uri.clone(),
                at: None,
                before: None,
                after: None,
            }]))
            .await
            .unwrap();
        let crate::OperationResult::Read(mut values) = full else {
            panic!("wrong result")
        };
        let crate::ReadResult::Text(text) = values.remove(0).unwrap() else {
            panic!("wrong read")
        };
        assert_eq!(
            text.lines
                .iter()
                .map(|line| &line.ending)
                .collect::<Vec<_>>(),
            vec![
                &crate::LineEnding::Crlf,
                &crate::LineEnding::Cr,
                &crate::LineEnding::Lf,
                &crate::LineEnding::None
            ]
        );
        let anchor = text.lines[1].anchor.clone();
        let window = kernel
            .execute_operation(crate::Operation::Read(vec![crate::ReadRequest {
                uri,
                at: Some(crate::Position::At(anchor)),
                before: Some(1),
                after: Some(1),
            }]))
            .await
            .unwrap();
        let crate::OperationResult::Read(mut values) = window else {
            panic!("wrong result")
        };
        let crate::ReadResult::Text(text) = values.remove(0).unwrap() else {
            panic!("wrong read")
        };
        assert_eq!(
            text.lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two", "three"]
        );
    }

    #[tokio::test]
    async fn typed_delete_rejects_nonempty_directories() {
        let root = tempdir().unwrap();
        let directory = root.path().join("dir");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("child"), "x").unwrap();
        let uri = crate::ResourceUri::parse(&directory.display().to_string()).unwrap();
        let kernel = Kernel::new();
        kernel
            .register_typed(FileHandler::new(root.path()).unwrap())
            .await;
        let result = kernel
            .execute_operation(crate::Operation::Delete(vec![uri]))
            .await
            .unwrap();
        let crate::OperationResult::Delete(mut values) = result else {
            panic!("wrong result")
        };
        assert!(matches!(
            values.remove(0),
            Err(crate::KernelError::NotEmpty { .. })
        ));
    }

    #[tokio::test]
    async fn typed_read_applies_default_window_rejects_invalid_edges_and_marks_directories() {
        let root = tempdir().unwrap();
        let directory = root.path().join("dir");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("child"), "one\n").unwrap();
        fs::create_dir(directory.join("nested")).unwrap();
        let file = root.path().join("many.txt");
        fs::write(
            &file,
            (0..250).map(|i| format!("line{i}\n")).collect::<String>(),
        )
        .unwrap();
        let kernel = Kernel::new();
        kernel
            .register_typed(FileHandler::new(root.path()).unwrap())
            .await;

        let read = kernel
            .execute_operation(crate::Operation::Read(vec![crate::ReadRequest {
                uri: crate::ResourceUri::parse(&file.display().to_string()).unwrap(),
                at: None,
                before: None,
                after: None,
            }]))
            .await
            .unwrap();
        let crate::OperationResult::Read(mut values) = read else {
            panic!("wrong read result")
        };
        let Ok(crate::ReadResult::Text(text)) = values.remove(0) else {
            panic!("wrong read value")
        };
        assert_eq!(text.lines.len(), 201);

        let invalid = kernel
            .execute_operation(crate::Operation::Read(vec![crate::ReadRequest {
                uri: crate::ResourceUri::parse(&file.display().to_string()).unwrap(),
                at: Some(crate::Position::Top),
                before: Some(1),
                after: None,
            }]))
            .await
            .unwrap();
        assert!(matches!(
            invalid,
            crate::OperationResult::Read(values) if matches!(values[0], Err(crate::KernelError::InvalidRequest { .. }))
        ));

        let read_directory = kernel
            .execute_operation(crate::Operation::Read(vec![crate::ReadRequest {
                uri: crate::ResourceUri::parse(&directory.display().to_string()).unwrap(),
                at: None,
                before: None,
                after: None,
            }]))
            .await
            .unwrap();
        let crate::OperationResult::Read(mut values) = read_directory else {
            panic!("wrong directory result")
        };
        let Ok(crate::ReadResult::Directory { entries, .. }) = values.remove(0) else {
            panic!("wrong directory value")
        };
        assert!(entries.iter().any(|entry| entry.to_string().ends_with('/')));
    }
}
