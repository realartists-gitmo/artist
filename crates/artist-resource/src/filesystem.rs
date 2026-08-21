use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;

use crate::{
    ResourceError, ResourceOperation, ResourceProvider, ResourceReply, ResourceRequest,
    ResourceRoute, ResourceRouter, ResourceUri,
};

#[derive(Clone, Debug)]
pub struct FilesystemProvider {
    root: PathBuf,
}

impl FilesystemProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub async fn register(
        self: Arc<Self>,
        router: &ResourceRouter,
        plugin: impl Into<String>,
    ) -> Result<(), ResourceError> {
        router
            .register(
                plugin,
                ResourceRoute::new(
                    "file:///**",
                    None::<String>,
                    [
                        ResourceOperation::Read,
                        ResourceOperation::Children,
                        ResourceOperation::Write,
                        ResourceOperation::Move,
                    ],
                ),
                self,
            )
            .await
    }

    fn path(&self, uri: &ResourceUri) -> Result<PathBuf, ResourceError> {
        let path = uri
            .file_path()
            .ok_or_else(|| ResourceError::Invalid(format!("not a file URI: {uri}")))?;
        if path.starts_with(&self.root) {
            Ok(path)
        } else {
            Err(ResourceError::Invalid(format!(
                "file URI is outside provider root: {uri}"
            )))
        }
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
            ResourceRequest::Edit { uri, .. } => Err(ResourceError::Unsupported {
                uri,
                operation: ResourceOperation::Edit,
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
                })
                .await,
            Err(ResourceError::Unsupported {
                operation: ResourceOperation::Poll,
                ..
            })
        ));
    }
}
