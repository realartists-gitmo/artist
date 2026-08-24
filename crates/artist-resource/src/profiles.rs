use std::path::{Path, PathBuf};

use async_trait::async_trait;
use percent_encoding::percent_decode_str;

use crate::{
    ResourceError, ResourceOperation, ResourceProvider, ResourceReply, ResourceRequest, ResourceUri,
};

/// Native mechanics backing the logical `profiles:///` tree.
#[derive(Clone, Debug)]
pub struct ProfilesProvider {
    root: PathBuf,
}

impl ProfilesProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, uri: &ResourceUri) -> Result<PathBuf, ResourceError> {
        let url = uri.as_url();
        if url.scheme() != "profiles" || url.host_str().is_some() || url.query().is_some() {
            return Err(ResourceError::Invalid(format!(
                "expected a profiles:/// URI, got {uri}"
            )));
        }
        let mut path = self.root.clone();
        for encoded in url.path().split('/').filter(|segment| !segment.is_empty()) {
            let segment = percent_decode_str(encoded)
                .decode_utf8()
                .map_err(|_| ResourceError::Invalid("profile path is not UTF-8".into()))?;
            if segment == "." || segment == ".." || segment.contains(['/', '\\']) {
                return Err(ResourceError::Invalid(
                    "invalid profile path segment".into(),
                ));
            }
            path.push(segment.as_ref());
        }
        Ok(path)
    }

    fn uri(&self, path: &Path) -> Result<ResourceUri, ResourceError> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| ResourceError::Provider("profile path escaped its root".into()))?;
        let encoded = relative
            .components()
            .map(|component| {
                percent_encoding::utf8_percent_encode(
                    &component.as_os_str().to_string_lossy(),
                    percent_encoding::NON_ALPHANUMERIC,
                )
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("/");
        ResourceUri::resolve(&format!("profiles:///{encoded}"), Path::new("/"))
            .map_err(|error| ResourceError::Provider(error.to_string()))
    }
}

#[async_trait]
impl ResourceProvider for ProfilesProvider {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        match request {
            ResourceRequest::Read {
                uri,
                start_line,
                line_count,
            } => {
                let text = tokio::fs::read_to_string(self.path(&uri)?)
                    .await
                    .map_err(|error| provider(error, uri.clone(), ResourceOperation::Read))?;
                Ok(ResourceReply::Text {
                    text: slice_lines(&text, start_line, line_count),
                })
            }
            ResourceRequest::Children { uri } => {
                let catalog_root = uri.as_url().path() == "/";
                let mut directory = tokio::fs::read_dir(self.path(&uri)?)
                    .await
                    .map_err(|error| provider(error, uri.clone(), ResourceOperation::Children))?;
                let mut children = Vec::new();
                while let Some(entry) = directory
                    .next_entry()
                    .await
                    .map_err(|error| provider(error, uri.clone(), ResourceOperation::Children))?
                {
                    if catalog_root
                        && !entry
                            .file_type()
                            .await
                            .map_err(|error| {
                                provider(error, uri.clone(), ResourceOperation::Children)
                            })?
                            .is_dir()
                    {
                        continue;
                    }
                    children.push(self.uri(&entry.path())?);
                }
                children.sort();
                Ok(ResourceReply::Children { children })
            }
            other => Err(ResourceError::Unsupported {
                uri: other.uri().clone(),
                operation: other.operation(),
            }),
        }
    }
}

fn provider(
    error: std::io::Error,
    uri: ResourceUri,
    operation: ResourceOperation,
) -> ResourceError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ResourceError::NotFound { uri, operation }
    } else {
        ResourceError::Provider(error.to_string())
    }
}

fn slice_lines(text: &str, start_line: Option<u64>, line_count: Option<u64>) -> String {
    if start_line.is_none() && line_count.is_none() {
        return text.to_owned();
    }
    let start = start_line.unwrap_or(1).saturating_sub(1) as usize;
    let count = line_count.unwrap_or(u64::MAX) as usize;
    text.split_inclusive('\n').skip(start).take(count).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn projects_profile_directories_as_profiles_uris() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("planner");
        std::fs::create_dir(&profile).unwrap();
        std::fs::write(profile.join("instructions.md"), "Plan only.\n").unwrap();
        let provider = ProfilesProvider::new(temp.path());
        let root = ResourceUri::resolve("profiles:///", Path::new("/")).unwrap();
        let ResourceReply::Children { children } = provider
            .handle(ResourceRequest::Children { uri: root })
            .await
            .unwrap()
        else {
            panic!("expected children");
        };
        assert_eq!(children[0].to_string(), "profiles:///planner");
    }
}
