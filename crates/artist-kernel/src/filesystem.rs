use crate::{
    Anchor, AnchorError, AnchorSet, AnchoredLine, AnchoredText, ClaimDecision,
    DynamicClaimProvider, DynamicResourceProvider, DynamicValue, DynamicVerbResult, EditOperation,
    InsertOperation, InsertionPoint, KernelError, Pattern, Position, ReplaceOperation,
    ResourceFuture, ResourceUri, SearchService, StructuralAnalyzer, StructuralLine, VerbId,
};
use cap_std::{ambient_authority, fs::Dir};
#[cfg(test)]
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Dynamic identities implemented by a filesystem resource provider.
///
/// The identities are supplied by the active verb package catalog; the
/// provider contains no closed-world verb enum and can therefore be reused by
/// a package with a different canonical identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileVerbBindings {
    pub read: VerbId,
    pub write: VerbId,
    pub edit: VerbId,
    pub delete: VerbId,
    pub find: VerbId,
    pub grep: VerbId,
}

/// Adapter exposing native filesystem semantics through the open-ended
/// dynamic resource ABI. JSON conversion remains above this boundary.
pub struct FileResourceProvider {
    handler: Arc<FileHandler>,
    bindings: FileVerbBindings,
}

impl FileResourceProvider {
    pub fn new(handler: Arc<FileHandler>, bindings: FileVerbBindings) -> Self {
        Self { handler, bindings }
    }

    pub fn handler(&self) -> &Arc<FileHandler> {
        &self.handler
    }

    pub fn bindings(&self) -> &FileVerbBindings {
        &self.bindings
    }
}

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
}

#[derive(Debug, Deserialize)]
#[cfg(test)]
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

impl DynamicClaimProvider for FileResourceProvider {
    fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
        let supported = [
            &self.bindings.read,
            &self.bindings.write,
            &self.bindings.edit,
            &self.bindings.delete,
            &self.bindings.find,
            &self.bindings.grep,
        ]
        .iter()
        .any(|candidate| *candidate == verb);
        if supported
            && uri.scheme() == "file"
            && uri.query().is_none()
            && !self.handler.is_virtual_projection(uri)
            && self.handler.typed_path_syntax(uri).is_ok()
        {
            ClaimDecision::Handle
        } else {
            ClaimDecision::Pass
        }
    }
}

impl DynamicResourceProvider for FileResourceProvider {
    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a> {
        Box::pin(async move {
            let output = if verb == &self.bindings.read {
                self.handler.dynamic_read(
                    uri.clone(),
                    dynamic_position(&input)?,
                    dynamic_u32_field(&input, "before")?,
                    dynamic_u32_field(&input, "after")?,
                )?
            } else if verb == &self.bindings.write {
                self.handler
                    .dynamic_write(uri.clone(), dynamic_string_field(&input, "content")?)?
            } else if verb == &self.bindings.edit {
                self.handler
                    .dynamic_edit(uri.clone(), dynamic_edit_operations(&input)?)?
            } else if verb == &self.bindings.delete {
                self.handler.dynamic_delete(uri.clone())?
            } else if verb == &self.bindings.find {
                self.handler
                    .dynamic_find(uri.clone(), dynamic_string_field(&input, "query")?)?
            } else if verb == &self.bindings.grep {
                self.handler
                    .dynamic_grep(uri.clone(), dynamic_string_field(&input, "pattern")?)?
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

impl FileHandler {
    fn dynamic_read(
        &self,
        uri: ResourceUri,
        at: Option<Position>,
        before: Option<u32>,
        after: Option<u32>,
    ) -> Result<DynamicValue, KernelError> {
        let path = self.typed_path(&uri)?;
        if self.cap_metadata(&path)?.is_dir() {
            let mut entries = self
                .cap_read_dir(&path)?
                .map(|entry| {
                    let entry = entry.map_err(|error| KernelError::Handler {
                        message: error.to_string(),
                    })?;
                    let child = path.join(entry.file_name());
                    let is_dir = entry
                        .file_type()
                        .map_err(|error| KernelError::Handler {
                            message: error.to_string(),
                        })?
                        .is_dir();
                    ResourceUri::parse(&format!(
                        "{}{}",
                        child.display(),
                        if is_dir { "/" } else { "" }
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?;
            entries.sort_by_key(ToString::to_string);
            Ok(DynamicValue::Record(BTreeMap::from([
                ("uri".to_owned(), DynamicValue::ResourceUri(uri)),
                (
                    "entries".to_owned(),
                    DynamicValue::List(
                        entries.into_iter().map(DynamicValue::ResourceUri).collect(),
                    ),
                ),
            ])))
        } else {
            let bytes = self.cap_read(&path)?;
            let text = self.anchored_text(uri, &path, &bytes)?;
            Ok(dynamic_text(select_read_window(
                text,
                at.as_ref(),
                before,
                after,
            )?))
        }
    }

    fn dynamic_write(
        &self,
        uri: ResourceUri,
        content: String,
    ) -> Result<DynamicValue, KernelError> {
        let requested = uri
            .as_ref()
            .to_file_path()
            .map_err(|_| KernelError::InvalidUri {
                message: uri.to_string(),
            })?;
        let path = self.resolve_for_write(&requested)?;
        self.cap_atomic_replace(&path, content.as_bytes())?;
        let bytes = self.cap_read(&path)?;
        Ok(dynamic_text(self.anchored_text(uri, &path, &bytes)?))
    }

    fn dynamic_edit(
        &self,
        uri: ResourceUri,
        operations: Vec<EditOperation>,
    ) -> Result<DynamicValue, KernelError> {
        let path = self.typed_path(&uri)?;
        let result = self.apply_typed_edit(&path, uri, &operations)?;
        Ok(DynamicValue::Record(BTreeMap::from([
            ("text".to_owned(), dynamic_text(result.text)),
            ("diff".to_owned(), dynamic_diff(result.diff)),
        ])))
    }

    fn dynamic_delete(&self, uri: ResourceUri) -> Result<DynamicValue, KernelError> {
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
        Ok(DynamicValue::ResourceUri(uri))
    }

    fn dynamic_find(&self, uri: ResourceUri, query: String) -> Result<DynamicValue, KernelError> {
        let paths = self
            .search
            .find_files(&self.typed_path(&uri)?, &Pattern::parse(&query)?)?;
        let mut seen = std::collections::HashSet::new();
        Ok(DynamicValue::List(
            paths
                .into_iter()
                .filter(|path| seen.insert(path.clone()))
                .map(|path| {
                    ResourceUri::parse(&format!(
                        "{}{}",
                        path.display(),
                        if path.is_dir() { "/" } else { "" }
                    ))
                    .map(DynamicValue::ResourceUri)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    fn dynamic_grep(&self, uri: ResourceUri, pattern: String) -> Result<DynamicValue, KernelError> {
        let matches = self.search.grep_file(
            &self.typed_path(&uri)?,
            &Pattern::parse(&pattern)?,
            &self.structure,
        )?;
        Ok(DynamicValue::List(
            matches.into_iter().map(dynamic_text).collect(),
        ))
    }
}

fn dynamic_record(input: &DynamicValue) -> Result<&BTreeMap<String, DynamicValue>, KernelError> {
    match input {
        DynamicValue::Record(fields) => Ok(fields),
        _ => Err(KernelError::InvalidRequest {
            message: "filesystem dynamic input must be a record".to_owned(),
        }),
    }
}

fn dynamic_string_field(input: &DynamicValue, name: &str) -> Result<String, KernelError> {
    match dynamic_record(input)?.get(name) {
        Some(DynamicValue::String(value)) => Ok(value.clone()),
        _ => Err(KernelError::InvalidRequest {
            message: format!("filesystem dynamic input field {name} must be a string"),
        }),
    }
}

fn dynamic_u32_field(input: &DynamicValue, name: &str) -> Result<Option<u32>, KernelError> {
    match dynamic_record(input)?.get(name) {
        None | Some(DynamicValue::Option(None)) => Ok(None),
        Some(DynamicValue::Option(Some(value))) => match value.as_ref() {
            DynamicValue::U32(value) => Ok(Some(*value)),
            DynamicValue::S32(value) if *value >= 0 => Ok(Some(*value as u32)),
            _ => Err(KernelError::InvalidRequest {
                message: format!("filesystem dynamic input field {name} must be u32"),
            }),
        },
        Some(DynamicValue::U32(value)) => Ok(Some(*value)),
        Some(DynamicValue::S32(value)) if *value >= 0 => Ok(Some(*value as u32)),
        _ => Err(KernelError::InvalidRequest {
            message: format!("filesystem dynamic input field {name} must be option<u32>"),
        }),
    }
}

fn dynamic_position(input: &DynamicValue) -> Result<Option<Position>, KernelError> {
    let Some(value) = dynamic_record(input)?.get("at") else {
        return Ok(None);
    };
    let value = match value {
        DynamicValue::Option(None) => return Ok(None),
        DynamicValue::Option(Some(value)) => value.as_ref(),
        value => value,
    };
    match value {
        DynamicValue::String(value) if value == "top" => Ok(Some(Position::Top)),
        DynamicValue::String(value) if value == "bottom" => Ok(Some(Position::Bottom)),
        DynamicValue::String(value) => {
            Ok(Some(Position::At(Anchor::from_tokens(vec![value.clone()]))))
        }
        _ => Err(KernelError::InvalidRequest {
            message: "filesystem dynamic input field at must be a string".to_owned(),
        }),
    }
}

fn dynamic_anchor(value: &DynamicValue) -> Result<Anchor, KernelError> {
    let tokens = match value {
        DynamicValue::String(value) => value
            .trim_start_matches('#')
            .split('.')
            .map(str::to_owned)
            .collect(),
        DynamicValue::List(values) => values
            .iter()
            .map(|value| match value {
                DynamicValue::String(value) => Ok(value.clone()),
                _ => Err(KernelError::InvalidRequest {
                    message: "anchor tokens must be strings".to_owned(),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(KernelError::InvalidRequest {
                message: "anchor must be a string or list<string>".to_owned(),
            });
        }
    };
    if tokens.is_empty() {
        return Err(KernelError::InvalidRequest {
            message: "anchor cannot be empty".to_owned(),
        });
    }
    Ok(Anchor::from_tokens(tokens))
}

fn dynamic_optional_anchor(
    fields: &BTreeMap<String, DynamicValue>,
    name: &str,
) -> Result<Option<Anchor>, KernelError> {
    match fields.get(name) {
        None | Some(DynamicValue::Option(None)) => Ok(None),
        Some(DynamicValue::Option(Some(value))) => dynamic_anchor(value).map(Some),
        Some(value) => dynamic_anchor(value).map(Some),
    }
}

fn dynamic_insertion_point(value: &DynamicValue) -> Result<InsertionPoint, KernelError> {
    let DynamicValue::Variant(name, value) = value else {
        return Err(KernelError::InvalidRequest {
            message: "edit insertion point must be a variant".to_owned(),
        });
    };
    match (name.as_str(), value.as_deref()) {
        ("top", None) => Ok(InsertionPoint::Top),
        ("bottom", None) => Ok(InsertionPoint::Bottom),
        ("before", Some(value)) => Ok(InsertionPoint::Before(dynamic_anchor(value)?)),
        ("after", Some(value)) => Ok(InsertionPoint::After(dynamic_anchor(value)?)),
        _ => Err(KernelError::InvalidRequest {
            message: format!("invalid edit insertion point variant {name}"),
        }),
    }
}

fn dynamic_edit_operations(input: &DynamicValue) -> Result<Vec<EditOperation>, KernelError> {
    let Some(DynamicValue::List(values)) = dynamic_record(input)?.get("operations") else {
        return Err(KernelError::InvalidRequest {
            message: "filesystem edit input requires operations list".to_owned(),
        });
    };
    values
        .iter()
        .map(|value| {
            let DynamicValue::Variant(name, payload) = value else {
                return Err(KernelError::InvalidRequest {
                    message: "edit operation must be a variant".to_owned(),
                });
            };
            let Some(DynamicValue::Record(fields)) = payload.as_deref() else {
                return Err(KernelError::InvalidRequest {
                    message: "edit operation variant requires a record".to_owned(),
                });
            };
            match name.as_str() {
                "replace" => Ok(EditOperation::Replace(ReplaceOperation {
                    start: dynamic_anchor(fields.get("start").ok_or_else(|| {
                        KernelError::InvalidRequest {
                            message: "replace requires start".to_owned(),
                        }
                    })?)?,
                    end: dynamic_optional_anchor(fields, "end")?,
                    content: dynamic_string_field(
                        &DynamicValue::Record(fields.clone()),
                        "content",
                    )?,
                })),
                "insert" => Ok(EditOperation::Insert(InsertOperation {
                    at: dynamic_insertion_point(fields.get("at").ok_or_else(|| {
                        KernelError::InvalidRequest {
                            message: "insert requires at".to_owned(),
                        }
                    })?)?,
                    content: dynamic_string_field(
                        &DynamicValue::Record(fields.clone()),
                        "content",
                    )?,
                })),
                _ => Err(KernelError::InvalidRequest {
                    message: format!("unknown edit operation variant {name}"),
                }),
            }
        })
        .collect()
}

fn dynamic_line(line: AnchoredLine) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        (
            "anchor".to_owned(),
            DynamicValue::List(
                line.anchor
                    .tokens()
                    .iter()
                    .cloned()
                    .map(DynamicValue::String)
                    .collect(),
            ),
        ),
        ("text".to_owned(), DynamicValue::String(line.text)),
        (
            "ending".to_owned(),
            DynamicValue::String(format!("{:?}", line.ending).to_lowercase()),
        ),
    ]))
}

fn dynamic_text(text: AnchoredText) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        ("uri".to_owned(), DynamicValue::ResourceUri(text.uri)),
        (
            "lines".to_owned(),
            DynamicValue::List(text.lines.into_iter().map(dynamic_line).collect()),
        ),
    ]))
}

fn dynamic_diff(diff: crate::AnchoredDiff) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        ("uri".to_owned(), DynamicValue::ResourceUri(diff.uri)),
        (
            "hunks".to_owned(),
            DynamicValue::List(
                diff.hunks
                    .into_iter()
                    .map(|hunk| {
                        DynamicValue::Record(BTreeMap::from([
                            (
                                "old".to_owned(),
                                DynamicValue::List(
                                    hunk.old.into_iter().map(dynamic_line).collect(),
                                ),
                            ),
                            (
                                "new".to_owned(),
                                DynamicValue::List(
                                    hunk.new.into_iter().map(dynamic_line).collect(),
                                ),
                            ),
                        ]))
                    })
                    .collect(),
            ),
        ),
    ]))
}
