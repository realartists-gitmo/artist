//! Resource-provider projection of the durable scoped store. Plugins and
//! profiles address durable domain state through ordinary `store://` URIs;
//! they never import a generic key/value database.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use crate::resource::{ResourceProvider, ResourceReply, ResourceRequest};
use crate::storage::{FileResourceStore, MemoryResourceStore, ScopedResourceStore, StorageError};
use crate::uri::ResourceUri;

/// The owner-qualified scope segment of a `store://` URI.
///
/// `store://global/<key>` maps to scope `global`;
/// `store://account/<name>/<key>` to `account/<name>`; likewise `identity`,
/// `workspace`, `profile`, and `session/<id>`. Scopes are explicit URI
/// segments — nothing is ever inferred from key prefixes.
fn split_scope_and_key(path: &str) -> Result<(String, String), String> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // Keys are flat tokens within a scope (the file backend hashes them into
    // object names), so exactly one key segment follows the scope.
    if segments.len() < 2 {
        return Err("store URIs need a scope segment and a key: store:///<scope>/<key>".into());
    }
    let (scope_segments, key_segments) = match segments[0] {
        "global" => (&segments[..1], &segments[1..]),
        "session" | "account" | "identity" | "workspace" | "profile" => {
            if segments.len() < 3 {
                return Err(format!(
                    "store scope `{}` needs an owner id and a key",
                    segments[0]
                ));
            }
            (&segments[..2], &segments[2..])
        }
        other => {
            return Err(format!(
                "unknown store scope `{other}`; expected global, session, account, identity, workspace, or profile"
            ));
        }
    };
    if key_segments.len() != 1 || key_segments[0].contains('/') {
        return Err(format!(
            "store keys are single path segments within a scope; got `{}`",
            key_segments[0]
        ));
    }
    // Scope tokens stay single-segment for the backend; `:` joins kind and
    // owner (`account:acme`).
    let scope_token = if scope_segments.len() == 1 {
        scope_segments[0].to_string()
    } else {
        format!("{}:{}", scope_segments[0], scope_segments[1])
    };
    Ok((scope_token, key_segments[0].to_string()))
}

/// Durable documents projected as ordinary resources.
pub struct StorageProvider {
    store: Arc<dyn ScopedResourceStore>,
}

impl StorageProvider {
    pub fn shared(store: Arc<dyn ScopedResourceStore>) -> Self {
        Self { store }
    }

    /// File-backed, restart-durable default rooted at the host working
    /// directory.
    pub fn open(root: impl Into<std::path::PathBuf>) -> Result<Self, StorageError> {
        Ok(Self::shared(Arc::new(FileResourceStore::open(root)?)))
    }

    /// In-memory variant for tests.
    pub fn memory() -> Self {
        Self::shared(Arc::new(MemoryResourceStore::default()))
    }
}

#[async_trait]
impl ResourceProvider for StorageProvider {
    async fn handle(
        &self,
        request: ResourceRequest,
    ) -> Result<ResourceReply, crate::resource::ResourceError> {
        let provider_error = |message: String| {
            crate::resource::ResourceError::Provider(format!("durable store: {message}"))
        };
        let storage_error = |error: StorageError| provider_error(error.to_string());
        match request {
            ResourceRequest::Read { uri, .. } => {
                let (scope, key) = split_scope_and_key(&path_of(&uri)).map_err(provider_error)?;
                let document = self.store.read(&scope, &key).await.map_err(storage_error)?;
                let Some(document) = document else {
                    return Err(crate::resource::ResourceError::Invalid(format!(
                        "durable document not found: {uri}"
                    )));
                };
                let text = String::from_utf8(document.value).map_err(|_| {
                    crate::resource::ResourceError::Invalid(
                        "durable document is not UTF-8 text".into(),
                    )
                })?;
                Ok(ResourceReply::Text { text })
            }
            ResourceRequest::Write { uri, text } => {
                let (scope, key) = split_scope_and_key(&path_of(&uri)).map_err(provider_error)?;
                self.store
                    .write(&scope, &key, text.into_bytes(), None)
                    .await
                    .map_err(storage_error)?;
                Ok(ResourceReply::Written)
            }
            ResourceRequest::Move { from, to } => {
                let (scope, key) = split_scope_and_key(&path_of(&from)).map_err(provider_error)?;
                if let Some(to) = to {
                    let (to_scope, to_key) =
                        split_scope_and_key(&path_of(&to)).map_err(provider_error)?;
                    let document = self
                        .store
                        .read(&scope, &key)
                        .await
                        .map_err(storage_error)?
                        .ok_or_else(|| {
                            provider_error(format!("durable document not found: {from}"))
                        })?;
                    self.store
                        .write(&to_scope, &to_key, document.value, None)
                        .await
                        .map_err(storage_error)?;
                    self.store
                        .delete(&scope, &key, None)
                        .await
                        .map_err(storage_error)?;
                } else {
                    let deleted = self
                        .store
                        .delete(&scope, &key, None)
                        .await
                        .map_err(storage_error)?;
                    if !deleted {
                        return Err(crate::resource::ResourceError::Invalid(format!(
                            "durable document not found: {from}"
                        )));
                    }
                }
                Ok(ResourceReply::Moved)
            }
            ResourceRequest::Children { uri } => {
                let path = path_of(&uri);
                let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                // Children of a scope root lists that scope's keys; children
                // of a named-scope owner (e.g. store://account/acme) lists the
                // keys under every key-prefix one level deeper is not
                // supported — scopes are flat namespaces.
                let scope = match segments.as_slice() {
                    [only]
                        if *only == "global"
                            || *only == "session"
                            || *only == "account"
                            || *only == "identity"
                            || *only == "workspace"
                            || *only == "profile" =>
                    {
                        if *only == "global" {
                            "global".to_string()
                        } else {
                            return Err(crate::resource::ResourceError::Invalid(format!(
                                "store scope `{only}` needs an owner id"
                            )));
                        }
                    }
                    [kind, owner]
                        if matches!(
                            *kind,
                            "session" | "account" | "identity" | "workspace" | "profile"
                        ) =>
                    {
                        format!("{kind}:{owner}")
                    }
                    _ => {
                        return Err(crate::resource::ResourceError::Invalid(
                            "children are listed at scope roots only".into(),
                        ));
                    }
                };
                let documents = self.store.list(&scope).await.map_err(storage_error)?;
                let children = documents
                    .iter()
                    .map(|document| {
                        let prefix = if scope == "global" {
                            String::new()
                        } else {
                            format!("{}/", scope)
                        };
                        ResourceUri::resolve(
                            &format!("store:///{prefix}{}", document.key),
                            Path::new("/"),
                        )
                        .map_err(|error| provider_error(error.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ResourceReply::Children { children })
            }
            other => Err(crate::resource::ResourceError::Invalid(format!(
                "durable store does not support {}",
                operation_name(&other)
            ))),
        }
    }
}

fn path_of(uri: &ResourceUri) -> String {
    uri.as_url().path().trim_start_matches('/').to_string()
}

fn operation_name(request: &ResourceRequest) -> String {
    match request {
        ResourceRequest::Read { .. } => "read options".into(),
        ResourceRequest::Edit { .. } => "edit".into(),
        ResourceRequest::Run { .. } => "run".into(),
        ResourceRequest::Signal { .. } => "signal".into(),
        ResourceRequest::Poll { .. } => "poll".into(),
        _ => "the requested operation".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::ResourceError;
    use std::path::PathBuf;

    fn uri(path: &str) -> ResourceUri {
        ResourceUri::resolve(path, Path::new("/")).unwrap()
    }

    async fn read(provider: &StorageProvider, path: &str) -> Result<String, ResourceError> {
        match provider
            .handle(ResourceRequest::Read {
                uri: uri(path),
                start_line: None,
                line_count: None,
            })
            .await?
        {
            ResourceReply::Text { text } => Ok(text),
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[tokio::test]
    async fn documents_round_trip_through_scope_uris() {
        let provider = StorageProvider::memory();
        provider
            .handle(ResourceRequest::Write {
                uri: uri("store:///global/notes-list"),
                text: "[]".into(),
            })
            .await
            .unwrap();
        provider
            .handle(ResourceRequest::Write {
                uri: uri("store:///account/acme/config"),
                text: "team".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            read(&provider, "store:///global/notes-list").await.unwrap(),
            "[]"
        );
        assert_eq!(
            read(&provider, "store:///account/acme/config")
                .await
                .unwrap(),
            "team"
        );

        // Same key text in different scopes stays independent.
        assert!(matches!(
            read(&provider, "store:///workspace/acme/config").await,
            Err(ResourceError::Invalid(_))
        ));

        // Children at a scope root list that scope's keys.
        match provider
            .handle(ResourceRequest::Children {
                uri: uri("store:///global"),
            })
            .await
            .unwrap()
        {
            ResourceReply::Children { children } => {
                let rendered: Vec<String> = children
                    .iter()
                    .map(|child| child.as_url().to_string())
                    .collect();
                assert!(
                    rendered
                        .iter()
                        .any(|child| child.ends_with("/global/notes-list")
                            || child.ends_with("notes-list")),
                    "children did not include the document: {rendered:?}"
                );
            }
            other => panic!("unexpected reply: {other:?}"),
        }

        // Move-to-None deletes.
        provider
            .handle(ResourceRequest::Move {
                from: uri("store:///global/notes-list"),
                to: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            read(&provider, "store:///global/notes-list").await,
            Err(ResourceError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn file_backed_documents_survive_a_provider_restart() {
        let root = tempfile::tempdir().unwrap();
        let path: PathBuf = root.path().join("durable-store");
        {
            let provider = StorageProvider::open(&path).unwrap();
            provider
                .handle(ResourceRequest::Write {
                    uri: uri("store:///session/s-1/state"),
                    text: "persisted".into(),
                })
                .await
                .unwrap();
        }
        let reopened = StorageProvider::open(&path).unwrap();
        assert_eq!(
            read(&reopened, "store:///session/s-1/state").await.unwrap(),
            "persisted"
        );
    }
}
