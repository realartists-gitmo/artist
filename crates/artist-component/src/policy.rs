//! Default policy components built on the generic resource/component seams.
//!
//! These are deliberately small host-side implementations of the contracts
//! that the replaceable WASM prompt/profile/permission extensions will use.
//! Keeping the contracts here makes the behavior testable before those
//! policies are moved into independently shipped component packages.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use artist_kernel::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use artist_kernel::{Kernel, ResourceUri};
use async_trait::async_trait;

/// Read-only host directory used as one layer of a logical model-facing view.
/// The kernel's `LayeredResourceProvider` composes two instances of this type.
pub struct DirectoryResourceProvider {
    name: String,
    scheme: String,
    root: PathBuf,
}

impl DirectoryResourceProvider {
    pub fn new(
        name: impl Into<String>,
        scheme: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            scheme: scheme.into(),
            root: root.into(),
        }
    }

    fn path(&self, uri: &ResourceUri) -> Result<PathBuf, ResourceError> {
        if uri.scheme() != self.scheme || uri.query().is_some() || uri.fragment().is_some() {
            return Err(ResourceError::not_found(uri));
        }
        let mut path = self.root.clone();
        let mut segments = Vec::new();
        if !uri.authority().is_empty() {
            segments.push(uri.authority().to_owned());
        }
        segments.extend(
            uri.decoded_segments()
                .map_err(|_| {
                    ResourceError::new(ResourceErrorCode::InvalidAddress, "invalid provider URI")
                })?
                .into_iter()
                .filter(|segment| !segment.is_empty()),
        );
        for segment in segments {
            if segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.contains('/')
                || segment.contains('\\')
            {
                return Err(ResourceError::new(
                    ResourceErrorCode::InvalidAddress,
                    "invalid provider path",
                ));
            }
            path.push(segment);
        }
        Ok(path)
    }

    fn attrs_for(uri: &ResourceUri, path: &Path) -> Result<ProviderAttrs, ResourceError> {
        let metadata = fs::metadata(path).map_err(|error| map_io(uri, error))?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if metadata.is_dir() {
            Ok(ProviderAttrs::directory())
        } else {
            Ok(ProviderAttrs::file(metadata.len(), modified))
        }
    }
}

#[async_trait]
impl ResourceProvider for DirectoryResourceProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }
    fn eligible(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == self.scheme
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        Self::attrs_for(uri, &self.path(uri)?)
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        let path = self.path(uri)?;
        let metadata = fs::metadata(&path).map_err(|error| map_io(uri, error))?;
        if !metadata.is_dir() {
            return Err(ResourceError::new(
                ResourceErrorCode::NotDir,
                "resource is not a directory",
            ));
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(path).map_err(|error| map_io(uri, error))? {
            let entry = entry.map_err(|error| map_io(uri, error))?;
            let metadata = entry.metadata().map_err(|error| map_io(uri, error))?;
            entries.push(ProviderEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                attrs: if metadata.is_dir() {
                    ProviderAttrs::directory()
                } else {
                    ProviderAttrs::file(
                        metadata.len(),
                        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    )
                },
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        let path = self.path(uri)?;
        let mut file = File::open(&path).map_err(|error| map_io(uri, error))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| map_io(uri, error))?;
        let mut bytes = vec![0; size as usize];
        let count = file.read(&mut bytes).map_err(|error| map_io(uri, error))?;
        bytes.truncate(count);
        Ok(bytes)
    }
}

fn map_io(uri: &ResourceUri, error: std::io::Error) -> ResourceError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => ResourceErrorCode::NotFound,
        std::io::ErrorKind::PermissionDenied => ResourceErrorCode::PermissionDenied,
        _ => ResourceErrorCode::Io,
    };
    ResourceError::new(code, format!("resource provider failed for {uri}: {error}"))
}

/// Install the ordinary profile view from global and project-local sources.
pub fn install_profile_view(
    kernel: &Kernel,
    global: impl Into<PathBuf>,
    local: impl Into<PathBuf>,
) {
    kernel.register_layered_resource_provider(
        "profile",
        DirectoryResourceProvider::new("profile-local", "profile", local),
        DirectoryResourceProvider::new("profile-global", "profile", global),
    );
}

/// Install a prompt view. Prompt assets remain a component-owned namespace;
/// this helper only supplies the default read-only resource implementation.
pub fn install_prompt_view(kernel: &Kernel, global: impl Into<PathBuf>, local: impl Into<PathBuf>) {
    kernel.register_layered_resource_provider(
        "prompt",
        DirectoryResourceProvider::new("prompt-local", "prompt", local),
        DirectoryResourceProvider::new("prompt-global", "prompt", global),
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionEffect {
    Allow,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRule {
    pub effect: PermissionEffect,
    pub verb: Option<String>,
    pub pattern: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PermissionRegistry {
    rules: Arc<Vec<PermissionRule>>,
}

impl PermissionRegistry {
    pub fn new(rules: impl IntoIterator<Item = PermissionRule>) -> Self {
        Self {
            rules: Arc::new(rules.into_iter().collect()),
        }
    }

    pub fn authorize(&self, profile: &str, verb: &str, uri: &str) -> bool {
        let mut decision = true;
        for rule in self.rules.iter() {
            if rule.verb.as_deref().is_some_and(|value| value != verb) {
                continue;
            }
            let Some(raw_pattern) = &rule.pattern else {
                decision = matches!(rule.effect, PermissionEffect::Allow);
                continue;
            };
            let pattern = raw_pattern.replace("{current-profile}", profile);
            if wildcard_match(uri, &pattern)
                || (pattern.ends_with('/') && uri.starts_with(&pattern))
            {
                decision = matches!(rule.effect, PermissionEffect::Allow);
            }
        }
        decision
    }

    /// Interpret a query such as `?read` as an authorization metadata request.
    pub fn query(&self, profile: &str, uri: &str) -> Option<bool> {
        let parsed: ResourceUri = uri.parse().ok()?;
        let verb = parsed.query()?.trim_start_matches('?');
        let base = format!(
            "{}://{}{}",
            parsed.scheme(),
            parsed.authority(),
            parsed.path()
        );
        (!verb.is_empty()).then(|| self.authorize(profile, verb, &base))
    }
}

fn wildcard_match(value: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts: Vec<_> = pattern.split('*').collect();
    if parts.len() == 1 {
        return value == pattern;
    }
    let mut cursor = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let Some(found) = value[cursor..].find(part) else {
            return false;
        };
        if index == 0 && found != 0 {
            return false;
        }
        cursor += found + part.len();
    }
    pattern.ends_with('*') || value.ends_with(parts.last().copied().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn permissions_expand_current_profile_and_match_queries() {
        let registry = PermissionRegistry::new([PermissionRule {
            effect: PermissionEffect::Deny,
            verb: Some("read".into()),
            pattern: Some("profile://{current-profile}/.artist/".into()),
        }]);
        assert!(!registry.authorize("reviewer", "read", "profile://reviewer/.artist/tools/x"));
        assert!(registry.authorize("reviewer", "read", "profile://other/.artist/tools/x"));
        assert!(
            !registry
                .query("reviewer", "profile://reviewer/.artist/tools/x?read")
                .unwrap()
        );
    }

    #[tokio::test]
    async fn profile_view_is_local_over_global_and_flattens_directory_entries() {
        let dir = tempdir().unwrap();
        let global = dir.path().join("global");
        let local = dir.path().join("local");
        std::fs::create_dir_all(global.join("reviewer/.artist/tools")).unwrap();
        std::fs::create_dir_all(local.join("reviewer/.artist/tools")).unwrap();
        std::fs::write(global.join("reviewer/PROFILE.md"), "global").unwrap();
        std::fs::write(local.join("reviewer/PROFILE.md"), "local").unwrap();
        std::fs::write(global.join("reviewer/.artist/tools/a"), "a").unwrap();
        std::fs::write(local.join("reviewer/.artist/tools/b"), "b").unwrap();
        let kernel = Kernel::empty();
        install_profile_view(&kernel, global, local);
        let uri: ResourceUri = "profile://reviewer/PROFILE.md".parse().unwrap();
        assert_eq!(kernel.read_uri(&uri, 0, 100).await.unwrap(), b"local");
        let tools: ResourceUri = "profile://reviewer/.artist/tools".parse().unwrap();
        let entries = kernel.readdir_uri(&tools).await.unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }
}
