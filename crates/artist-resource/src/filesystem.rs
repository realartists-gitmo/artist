use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

use async_trait::async_trait;

use crate::{
    ResourceError, ResourceOperation, ResourceProvider, ResourceReply, ResourceRequest,
    ResourceUri, TextReplacement, apply_replacements, sha256,
};

#[derive(Clone, Debug, Default)]
pub struct FilesystemProvider {
    locks: Arc<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>>,
}

impl FilesystemProvider {
    /// Construct the native provider. `scope` is organizational context for
    /// callers; it is deliberately not a permission or confinement boundary.
    pub fn new(_scope: impl Into<PathBuf>) -> Self {
        Self::default()
    }

    fn path(&self, uri: &ResourceUri) -> Result<PathBuf, ResourceError> {
        let path = uri
            .file_path()
            .ok_or_else(|| ResourceError::Invalid(format!("not a file URI: {uri}")))?;
        Ok(path)
    }

    fn path_lock(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().expect("filesystem lock map poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(path.to_owned(), Arc::downgrade(&lock));
        lock
    }

    pub async fn edit(
        &self,
        uri: ResourceUri,
        expected_sha256: String,
        replacements: Vec<TextReplacement>,
    ) -> Result<ResourceReply, ResourceError> {
        if replacements.is_empty() {
            return Err(ResourceError::Invalid(
                "an edit must contain at least one replacement".into(),
            ));
        }
        if expected_sha256.len() != 64
            || !expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ResourceError::Invalid(
                "expected_sha256 must be 64 lowercase hexadecimal characters".into(),
            ));
        }
        let path = self.path(&uri)?;
        let path_lock = self.path_lock(&path);
        let _guard = path_lock.lock().await;
        tokio::task::spawn_blocking(move || {
            let original = std::fs::read_to_string(&path).map_err(provider)?;
            let current_revision = sha256(original.as_bytes());
            if current_revision != expected_sha256 {
                return Err(ResourceError::Conflict {
                    uri,
                    current_revision,
                });
            }
            let edited = apply_replacements(&original, &replacements)
                .map_err(|error| ResourceError::Invalid(error.to_string()))?;
            let parent = path.parent().ok_or_else(|| {
                ResourceError::Invalid(format!("file has no parent directory: {}", path.display()))
            })?;
            let permissions = std::fs::metadata(&path).map_err(provider)?.permissions();
            let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(provider)?;
            temporary
                .as_file()
                .set_permissions(permissions)
                .map_err(provider)?;
            temporary.write_all(edited.as_bytes()).map_err(provider)?;
            temporary.as_file().sync_all().map_err(provider)?;
            temporary
                .persist(&path)
                .map_err(|error| provider(error.error))?;
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(provider)?;
            Ok(ResourceReply::Edited {
                revision: sha256(edited.as_bytes()),
            })
        })
        .await
        .map_err(|error| ResourceError::Provider(format!("edit task failed: {error}")))?
    }
}

#[async_trait]
impl ResourceProvider for FilesystemProvider {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        match request {
            ResourceRequest::Read {
                uri,
                start_line,
                line_count,
            } => {
                let text = tokio::fs::read_to_string(self.path(&uri)?)
                    .await
                    .map_err(provider)?;
                Ok(ResourceReply::Text {
                    text: slice_lines(&text, start_line, line_count),
                })
            }
            ResourceRequest::Children { uri } => {
                let mut dir = tokio::fs::read_dir(self.path(&uri)?)
                    .await
                    .map_err(provider)?;
                let mut children = Vec::new();
                while let Some(entry) = dir.next_entry().await.map_err(provider)? {
                    children.push(
                        ResourceUri::resolve(&entry.path().to_string_lossy(), Path::new("/"))
                            .map_err(|e| ResourceError::Provider(e.to_string()))?,
                    );
                }
                children.sort();
                Ok(ResourceReply::Children { children })
            }
            ResourceRequest::Write { uri, text } => {
                let path = self.path(&uri)?;
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(provider)?;
                }
                tokio::fs::write(path, text).await.map_err(provider)?;
                Ok(ResourceReply::Written)
            }
            ResourceRequest::Move { from, to } => {
                let from = self.path(&from)?;
                if let Some(to) = to {
                    let to = self.path(&to)?;
                    if let Some(parent) = to.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(provider)?;
                    }
                    tokio::fs::rename(from, to).await.map_err(provider)?;
                } else if tokio::fs::metadata(&from).await.map_err(provider)?.is_dir() {
                    tokio::fs::remove_dir_all(from).await.map_err(provider)?;
                } else {
                    tokio::fs::remove_file(from).await.map_err(provider)?;
                }
                Ok(ResourceReply::Moved)
            }
            ResourceRequest::Poll { uri, .. } => Err(ResourceError::Unsupported {
                uri,
                operation: ResourceOperation::Poll,
            }),
            ResourceRequest::Edit {
                uri,
                expected_sha256,
                replacements,
            } => self.edit(uri, expected_sha256, replacements).await,
            ResourceRequest::Run { target, .. } => Err(ResourceError::Unsupported {
                uri: target,
                operation: ResourceOperation::Run,
            }),
            ResourceRequest::Signal { uri, .. } => Err(ResourceError::Unsupported {
                uri,
                operation: ResourceOperation::Signal,
            }),
        }
    }
}

fn slice_lines(text: &str, start: Option<u64>, count: Option<u64>) -> String {
    if start.is_none() && count.is_none() {
        return text.to_owned();
    }
    let start = start.unwrap_or(1).saturating_sub(1) as usize;
    text.split_inclusive('\n')
        .skip(start)
        .take(count.unwrap_or(u64::MAX) as usize)
        .collect()
}

fn provider(error: std::io::Error) -> ResourceError {
    ResourceError::Provider(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn reads_and_lists_nodes_independently() {
        let temp = tempdir().unwrap();
        tokio::fs::write(temp.path().join("a.txt"), "one\ntwo\nthree\n")
            .await
            .unwrap();
        let provider = FilesystemProvider::new(temp.path());
        let uri = ResourceUri::resolve("a.txt", temp.path()).unwrap();
        assert_eq!(
            provider
                .handle(ResourceRequest::Read {
                    uri: uri.clone(),
                    start_line: Some(2),
                    line_count: Some(1)
                })
                .await
                .unwrap(),
            ResourceReply::Text {
                text: "two\n".into()
            }
        );
        let root = ResourceUri::resolve(".", temp.path()).unwrap();
        assert!(
            matches!(provider.handle(ResourceRequest::Children { uri: root }).await.unwrap(), ResourceReply::Children { children } if children == vec![uri.clone()])
        );
        assert!(matches!(
            provider
                .handle(ResourceRequest::Poll {
                    uri,
                    pattern: None,
                    timeout: Some(std::time::Duration::ZERO),
                    cursor: None,
                })
                .await,
            Err(ResourceError::Unsupported {
                operation: ResourceOperation::Poll,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn compare_and_apply_is_atomic_and_reports_conflicts() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("a.txt");
        tokio::fs::write(&path, "one\ntwo\n").await.unwrap();
        let provider = FilesystemProvider::new(temp.path());
        let uri = ResourceUri::resolve("a.txt", temp.path()).unwrap();
        let revision = sha256(b"one\ntwo\n");
        assert_eq!(
            provider
                .edit(
                    uri.clone(),
                    revision.clone(),
                    vec![TextReplacement {
                        start_byte: 0,
                        end_byte: 3,
                        text: "ONE".into(),
                    }],
                )
                .await
                .unwrap(),
            ResourceReply::Edited {
                revision: sha256(b"ONE\ntwo\n")
            }
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "ONE\ntwo\n"
        );
        assert!(matches!(
            provider
                .edit(
                    uri.clone(),
                    revision,
                    vec![TextReplacement {
                        start_byte: 4,
                        end_byte: 7,
                        text: "TWO".into(),
                    }],
                )
                .await,
            Err(ResourceError::Conflict {
                current_revision,
                ..
            }) if current_revision == sha256(b"ONE\ntwo\n")
        ));
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "ONE\ntwo\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn edit_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let path = temp.path().join("script");
        tokio::fs::write(&path, "old").await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751))
            .await
            .unwrap();
        let provider = FilesystemProvider::new(temp.path());
        provider
            .edit(
                ResourceUri::resolve("script", temp.path()).unwrap(),
                sha256(b"old"),
                vec![TextReplacement {
                    start_byte: 0,
                    end_byte: 3,
                    text: "new".into(),
                }],
            )
            .await
            .unwrap();
        let mode = tokio::fs::metadata(path)
            .await
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o751);
    }

    #[tokio::test]
    async fn concurrent_artist_edits_serialize_and_one_conflicts() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("shared.txt");
        tokio::fs::write(&path, "old").await.unwrap();
        let provider = FilesystemProvider::new(temp.path());
        let uri = ResourceUri::resolve("shared.txt", temp.path()).unwrap();
        let edit = |text: &'static str| {
            let provider = provider.clone();
            let uri = uri.clone();
            tokio::spawn(async move {
                provider
                    .edit(
                        uri,
                        sha256(b"old"),
                        vec![TextReplacement {
                            start_byte: 0,
                            end_byte: 3,
                            text: text.into(),
                        }],
                    )
                    .await
            })
        };
        let (left, right) = tokio::join!(edit("left"), edit("right"));
        let results = [left.unwrap(), right.unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(ResourceError::Conflict { .. })))
                .count(),
            1
        );
        assert!(matches!(
            tokio::fs::read_to_string(path).await.unwrap().as_str(),
            "left" | "right"
        ));
    }
}
