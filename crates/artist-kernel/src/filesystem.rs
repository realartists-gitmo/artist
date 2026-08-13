use crate::{
    Anchor, AnchorError, AnchorSet, AnchoredLine, AnchoredText, BoxFuture, Handler,
    HandlerDescriptor, KernelError, KernelHandle, Operation, OperationResult, ReadResult, Request,
    ResourceAddress, StructuralAnalyzer, StructuralLine, TypedHandler, Verb, address::uri_path,
    fff, has_projection,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

/// Native filesystem handler constrained to one root directory.
pub struct FileHandler {
    root: PathBuf,
    structure: StructuralAnalyzer,
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
        Ok(Self {
            root,
            structure: StructuralAnalyzer::default(),
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

    fn typed_path(&self, uri: &crate::ResourceUri) -> Result<PathBuf, KernelError> {
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
        self.resolve_existing(&path)
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
                let ending = if bytes.get(line.end_byte) == Some(&b'\n') {
                    crate::LineEnding::Lf
                } else {
                    crate::LineEnding::None
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
        let candidate = if requested.is_absolute() {
            requested.to_owned()
        } else {
            self.root.join(requested)
        };
        let resolved = fs::canonicalize(&candidate).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                KernelError::NotFound {
                    uri: candidate.display().to_string(),
                }
            } else {
                KernelError::Handler {
                    message: format!("resolve {}: {error}", candidate.display()),
                }
            }
        })?;
        self.ensure_in_root(&resolved)?;
        Ok(resolved)
    }

    fn resolve_for_write(&self, requested: &Path) -> Result<PathBuf, KernelError> {
        let candidate = if requested.is_absolute() {
            requested.to_owned()
        } else {
            self.root.join(requested)
        };
        let parent = candidate
            .parent()
            .ok_or_else(|| KernelError::InvalidRequest {
                message: format!("path has no parent: {}", candidate.display()),
            })?;
        let parent = fs::canonicalize(parent).map_err(|error| KernelError::Handler {
            message: format!("resolve parent {}: {error}", parent.display()),
        })?;
        self.ensure_in_root(&parent)?;
        Ok(parent.join(
            candidate
                .file_name()
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: format!("path has no filename: {}", candidate.display()),
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

    fn read_value(path: &Path) -> Result<Value, KernelError> {
        let metadata = fs::metadata(path).map_err(|error| KernelError::Handler {
            message: format!("stat {}: {error}", path.display()),
        })?;
        if metadata.is_dir() {
            let mut entries = fs::read_dir(path)
                .map_err(|error| KernelError::Handler {
                    message: format!("read directory {}: {error}", path.display()),
                })?
                .map(|entry| {
                    let entry = entry.map_err(|error| KernelError::Handler {
                        message: format!("read directory entry: {error}"),
                    })?;
                    let file_type = entry.file_type().map_err(|error| KernelError::Handler {
                        message: format!("read entry type: {error}"),
                    })?;
                    Ok(json!({
                        "name": entry.file_name().to_string_lossy(),
                        "path": entry.path().to_string_lossy(),
                        "directory": file_type.is_dir(),
                    }))
                })
                .collect::<Result<Vec<_>, KernelError>>()?;
            entries.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
            return Ok(json!({"type": "directory", "entries": entries}));
        }

        let bytes = fs::read(path).map_err(|error| KernelError::Handler {
            message: format!("read {}: {error}", path.display()),
        })?;
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
        let original = fs::read(&path).map_err(|error| KernelError::Handler {
            message: format!("read {}: {error}", path.display()),
        })?;
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
        atomic_replace(&path, &rendered)?;
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

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), KernelError> {
    let permissions = fs::metadata(path)
        .map_err(|error| KernelError::Handler {
            message: format!("stat {}: {error}", path.display()),
        })?
        .permissions();
    let parent = path.parent().ok_or_else(|| KernelError::InvalidRequest {
        message: format!("path has no parent: {}", path.display()),
    })?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| KernelError::Handler {
        message: format!(
            "create temporary edit file in {}: {error}",
            parent.display()
        ),
    })?;
    temporary
        .write_all(bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| KernelError::Handler {
            message: format!("write temporary edit file: {error}"),
        })?;
    temporary
        .as_file()
        .set_permissions(permissions)
        .map_err(|error| KernelError::Handler {
            message: format!("preserve permissions for {}: {error}", path.display()),
        })?;
    temporary
        .persist(path)
        .map_err(|error| KernelError::Handler {
            message: format!("atomically replace {}: {}", path.display(), error.error),
        })?;
    Ok(())
}

impl FileHandler {
    fn apply_typed_edit(
        &self,
        path: &Path,
        uri: crate::ResourceUri,
        operations: &[crate::EditOperation],
    ) -> Result<crate::EditResult, KernelError> {
        let original = fs::read(path).map_err(|error| KernelError::Handler {
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
                            lines[index].end_byte
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
        atomic_replace(path, &rendered)?;
        let new = self.anchored_text(uri.clone(), path, &rendered)?;
        Ok(crate::EditResult {
            uri: uri.clone(),
            changed: vec![new.clone()],
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
        address
            .as_uri()
            .is_some_and(|uri| uri.scheme() == "file" && !has_projection(Path::new(uri.path())))
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
                Verb::Read => Self::read_value(&self.resolve_existing(&requested)?)
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
                    fs::write(&path, content).map_err(|error| KernelError::Handler {
                        message: format!("write {}: {error}", path.display()),
                    })?;
                    Ok(json!({"written": true, "path": path}))
                }
                Verb::Edit => self.edit(&requested, &request.args),
                Verb::Delete => {
                    let path = self.resolve_existing(&requested)?;
                    if path.is_dir() {
                        fs::remove_dir_all(&path)
                    } else {
                        fs::remove_file(&path)
                    }
                    .map_err(|error| KernelError::Handler {
                        message: format!("delete {}: {error}", path.display()),
                    })?;
                    Ok(json!({"deleted": true, "path": path}))
                }
                Verb::Find => {
                    let path = self.resolve_existing(&requested)?;
                    let files = fff::find(&path, "")?;
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
                    let matches = fff::grep(&path, pattern, &self.structure)?;
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
            verbs: vec![Verb::Read, Verb::Write, Verb::Edit, Verb::Find, Verb::Grep],
        }
    }

    fn claims_operation(&self, operation: &Operation) -> bool {
        match operation {
            Operation::Read(requests) => requests
                .iter()
                .all(|request| self.typed_path(&request.uri).is_ok()),
            Operation::Write(requests) => requests.iter().all(|request| {
                request.uri.scheme() == "file" && request.uri.as_ref().to_file_path().is_ok()
            }),
            Operation::Edit(requests) => requests.iter().all(|request| {
                request.uri.scheme() == "file" && request.uri.as_ref().to_file_path().is_ok()
            }),
            Operation::Find(request) => {
                !request.roots.is_empty()
                    && request.roots.iter().all(|uri| self.typed_path(uri).is_ok())
            }
            Operation::Grep(request) => {
                matches!(&request.source, crate::GrepSource::Resources(uris) if !uris.is_empty() && uris.iter().all(|uri| self.typed_path(uri).is_ok()))
            }
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
                            let path = self.typed_path(&request.uri)?;
                            if path.is_dir() {
                                let mut entries = fs::read_dir(&path)
                                    .map_err(|error| KernelError::Handler {
                                        message: error.to_string(),
                                    })?
                                    .map(|entry| {
                                        let path = entry
                                            .map_err(|error| KernelError::Handler {
                                                message: error.to_string(),
                                            })?
                                            .path();
                                        crate::ResourceUri::parse(&path.display().to_string())
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                entries.sort_by_key(ToString::to_string);
                                Ok(ReadResult::Directory {
                                    uri: request.uri,
                                    entries,
                                })
                            } else {
                                let bytes =
                                    fs::read(&path).map_err(|error| KernelError::Handler {
                                        message: error.to_string(),
                                    })?;
                                Ok(ReadResult::Text(self.anchored_text(
                                    request.uri,
                                    &path,
                                    &bytes,
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
                            if let Some(parent) = path.parent() {
                                fs::create_dir_all(parent).map_err(|error| {
                                    KernelError::Handler {
                                        message: error.to_string(),
                                    }
                                })?;
                            }
                            fs::write(&path, request.content).map_err(|error| {
                                KernelError::Handler {
                                    message: error.to_string(),
                                }
                            })?;
                            let bytes = fs::read(&path).map_err(|error| KernelError::Handler {
                                message: error.to_string(),
                            })?;
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
                Operation::Find(request) => {
                    let mut paths = Vec::new();
                    for root in request.roots {
                        paths.extend(fff::find(&self.typed_path(&root)?, &request.query)?);
                    }
                    paths.sort();
                    paths.dedup();
                    Ok(OperationResult::Find(
                        paths
                            .into_iter()
                            .map(|path| crate::ResourceUri::parse(&path.display().to_string()))
                            .collect(),
                    ))
                }
                Operation::Grep(request) => {
                    let crate::GrepSource::Resources(uris) = request.source else {
                        unreachable!()
                    };
                    let mut matches = Vec::new();
                    for uri in uris {
                        matches.extend(fff::grep(
                            &self.typed_path(&uri)?,
                            &request.pattern,
                            &self.structure,
                        )?);
                    }
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
            .execute(request(Verb::Find, root.path(), Value::Null))
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
                source: crate::GrepSource::Resources(vec![uri]),
            }))
            .await
            .unwrap();
        let crate::OperationResult::Grep(Ok(matches)) = grep else {
            panic!("wrong grep result")
        };
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].lines[0].text, "    let answer = 42;");
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
        assert_eq!(value.uri, uri);
        assert_eq!(value.diff.hunks.len(), 1);
        assert_eq!(value.changed[0].lines.len(), 3);
    }
}
