use std::path::{Path, PathBuf};

use tokio::runtime::Handle;

use crate::{
    ResourceReply, ResourceRequest, ResourceRouter, ResourceUri,
    search::{decode_mount_name, encode_mount_name},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MountNode {
    Root,
    Scheme(String),
    Resource { uri: ResourceUri, directory: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MountEntry {
    pub name: String,
    pub node: MountNode,
    pub directory: bool,
    pub size: u64,
}

/// Platform-independent view consumed by every native mount adapter. It owns
/// all URI traversal, projection naming, resource classification, and router
/// dispatch; adapters translate only native filesystem callbacks.
pub(crate) struct ResourceMountModel {
    router: ResourceRouter,
    runtime: Handle,
    mount_root: PathBuf,
}

impl ResourceMountModel {
    pub fn new(router: ResourceRouter, runtime: Handle, mount_root: PathBuf) -> Self {
        Self {
            router,
            runtime,
            mount_root,
        }
    }

    pub fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, crate::ResourceError> {
        self.runtime.block_on(self.router.handle(request))
    }

    pub fn schemes(&self) -> Vec<String> {
        self.router.schemes()
    }

    pub fn child_uri(&self, parent: &MountNode, name: &str) -> Option<ResourceUri> {
        match parent {
            MountNode::Root => None,
            MountNode::Scheme(scheme)
                if matches!(scheme.as_str(), "file" | "profiles" | "plugins") =>
            {
                let name = decode_mount_name(name).ok()?;
                ResourceUri::resolve(&format!("{scheme}:///{name}"), Path::new("/")).ok()
            }
            MountNode::Scheme(scheme) => {
                let name = decode_mount_name(name).ok()?;
                ResourceUri::resolve(&format!("{scheme}://{name}/"), Path::new("/")).ok()
            }
            MountNode::Resource { uri, .. } if name.starts_with('~') => uri
                .descend_projection(&decode_mount_name(&name[1..]).ok()?)
                .ok(),
            MountNode::Resource { uri, .. } if !uri.projection_segments().is_empty() => {
                uri.descend_projection(&decode_mount_name(name).ok()?).ok()
            }
            MountNode::Resource { uri, .. } if name.contains('~') => {
                let (base, projection) = name.split_once('~')?;
                let base = decode_mount_name(base).ok()?;
                let projection = decode_mount_name(projection).ok()?;
                let child = if uri.as_url().scheme() == "file" {
                    let path = uri.file_path()?.join(&base);
                    ResourceUri::resolve(&path.to_string_lossy(), Path::new("/")).ok()?
                } else {
                    let mut url = uri.as_url().clone();
                    url.path_segments_mut().ok()?.push(&base);
                    ResourceUri::from_url(url).ok()?
                };
                child.descend_projection(&projection).ok()
            }
            MountNode::Resource { uri, .. } if uri.as_url().scheme() == "file" => {
                let path = uri.file_path()?.join(decode_mount_name(name).ok()?);
                ResourceUri::resolve(&path.to_string_lossy(), Path::new("/")).ok()
            }
            MountNode::Resource { uri, .. } => {
                let mut url = uri.as_url().clone();
                url.path_segments_mut()
                    .ok()?
                    .push(&decode_mount_name(name).ok()?);
                ResourceUri::from_url(url).ok()
            }
        }
    }

    pub fn classify(&self, uri: ResourceUri) -> Option<MountEntry> {
        let (directory, size) = match self.handle(ResourceRequest::Children { uri: uri.clone() }) {
            Ok(ResourceReply::Children { .. }) => (true, 0),
            _ => match self.handle(ResourceRequest::Read {
                uri: uri.clone(),
                start_line: None,
                line_count: None,
            }) {
                Ok(ResourceReply::Text { text }) => (false, text.len() as u64),
                _ => return None,
            },
        };
        Some(MountEntry {
            name: String::new(),
            node: MountNode::Resource { uri, directory },
            directory,
            size,
        })
    }

    pub fn entries(&self, node: &MountNode) -> Vec<MountEntry> {
        let mut entries = match node {
            MountNode::Root => self
                .schemes()
                .into_iter()
                .map(|scheme| MountEntry {
                    name: scheme.clone(),
                    node: MountNode::Scheme(scheme),
                    directory: true,
                    size: 0,
                })
                .collect(),
            MountNode::Scheme(scheme) => {
                let Ok(root) = ResourceUri::resolve(&format!("{scheme}:///"), Path::new("/"))
                else {
                    return Vec::new();
                };
                let Ok(ResourceReply::Children { children }) =
                    self.handle(ResourceRequest::Children { uri: root })
                else {
                    return Vec::new();
                };
                children
                    .into_iter()
                    .filter(|child| child.file_path().as_deref() != Some(self.mount_root.as_path()))
                    .filter_map(|child| {
                        let mut entry = self.classify(child.clone())?;
                        let name = child
                            .as_url()
                            .host_str()
                            .filter(|host| !host.is_empty())
                            .map(str::to_owned)
                            .or_else(|| resource_leaf_name(&child))
                            .unwrap_or_default();
                        entry.name = encode_mount_name(&name);
                        Some(entry)
                    })
                    .collect()
            }
            MountNode::Resource { uri, .. } => {
                let Ok(ResourceReply::Children { children }) =
                    self.handle(ResourceRequest::Children { uri: uri.clone() })
                else {
                    return Vec::new();
                };
                let mut entries = Vec::new();
                for child in children {
                    let Some(mut entry) = self.classify(child.clone()) else {
                        continue;
                    };
                    let base_name = resource_leaf_name(&child).unwrap_or_default();
                    let encoded_base = encode_mount_name(&base_name);
                    entry.name = encoded_base.clone();
                    entries.push(entry);
                    if child.projection_segments().is_empty() {
                        for projection in self.router.projection_roots(&child) {
                            let Ok(projected) = child.descend_projection(&projection) else {
                                continue;
                            };
                            let Some(mut projected_entry) = self.classify(projected) else {
                                continue;
                            };
                            projected_entry.name =
                                format!("{encoded_base}~{}", encode_mount_name(&projection));
                            entries.push(projected_entry);
                        }
                    }
                }
                entries
            }
        };
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        entries
    }
}

fn resource_leaf_name(uri: &ResourceUri) -> Option<String> {
    if let Some(projection) = uri.projection_segments().last() {
        return Some(projection.clone());
    }
    if uri.as_url().scheme() == "file" {
        return uri
            .file_path()?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
    }
    uri.as_url()
        .path_segments()?
        .rfind(|segment| !segment.is_empty())
        .and_then(|segment| decode_mount_name(segment).ok())
}

/// Narrow seam between platform-independent resource semantics and a native
/// filesystem integration.
pub trait MountedResourceFilesystem: Send {
    fn root(&self) -> &Path;
}

pub struct PlatformMount {
    backend: Box<dyn MountedResourceFilesystem>,
}

impl PlatformMount {
    pub fn mount(router: ResourceRouter, runtime: Handle) -> Result<Self, std::io::Error> {
        Ok(Self {
            backend: platform::mount(router, runtime)?,
        })
    }

    pub fn root(&self) -> &Path {
        self.backend.root()
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    impl MountedResourceFilesystem for crate::fuse::FuseMount {
        fn root(&self) -> &Path {
            crate::fuse::FuseMount::root(self)
        }
    }

    pub fn mount(
        router: ResourceRouter,
        runtime: Handle,
    ) -> Result<Box<dyn MountedResourceFilesystem>, std::io::Error> {
        crate::fuse::FuseMount::mount(router, runtime)
            .map(|mount| Box::new(mount) as Box<dyn MountedResourceFilesystem>)
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use async_trait::async_trait;

    use super::*;
    use crate::{
        FilesystemProvider, ResourceError, ResourceOperation, ResourceProvider, ResourceRoute,
        uri_to_mount_path,
    };

    struct ConformanceProvider;

    struct NativeSymbols;

    #[async_trait]
    impl ResourceProvider for NativeSymbols {
        async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
            match request {
                ResourceRequest::Children { uri } if uri.projection_segments().len() == 1 => {
                    Ok(ResourceReply::Children {
                        children: vec![uri.descend_projection("symbol").unwrap()],
                    })
                }
                ResourceRequest::Read { uri, .. } => Ok(ResourceReply::Text {
                    text: format!("symbol at {uri}\n"),
                }),
                other => Err(ResourceError::Unsupported {
                    uri: other.uri().clone(),
                    operation: other.operation(),
                }),
            }
        }
    }

    #[async_trait]
    impl ResourceProvider for ConformanceProvider {
        async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
            match request {
                ResourceRequest::Children { uri } if uri.as_url().path() == "/" => {
                    Ok(ResourceReply::Children {
                        children: vec![test_uri("mem:///dir")],
                    })
                }
                ResourceRequest::Children { uri } if uri.as_url().path() == "/dir" => {
                    Ok(ResourceReply::Children {
                        children: vec![test_uri("mem:///dir/doc")],
                    })
                }
                ResourceRequest::Children { uri } if !uri.projection_segments().is_empty() => {
                    Ok(ResourceReply::Children {
                        children: Vec::new(),
                    })
                }
                ResourceRequest::Read { .. } => Ok(ResourceReply::Text {
                    text: "body".into(),
                }),
                ResourceRequest::Write { .. } => Ok(ResourceReply::Written),
                ResourceRequest::Move { .. } => Ok(ResourceReply::Moved),
                other => Err(ResourceError::Unsupported {
                    uri: other.uri().clone(),
                    operation: other.operation(),
                }),
            }
        }
    }

    fn test_uri(text: &str) -> ResourceUri {
        ResourceUri::resolve(text, Path::new("/")).unwrap()
    }

    /// This exact suite is target-neutral: Linux FUSE, macFUSE, and WinFsp
    /// adapters all consume this model rather than reimplementing semantics.
    #[test]
    fn shared_backend_resource_semantics_conformance() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let router = ResourceRouter::new();
        let provider: Arc<dyn ResourceProvider> = Arc::new(ConformanceProvider);
        runtime
            .block_on(router.register(
                "mem",
                ResourceRoute::new(
                    "mem:///**",
                    None::<String>,
                    [
                        ResourceOperation::Read,
                        ResourceOperation::Children,
                        ResourceOperation::Write,
                        ResourceOperation::Move,
                    ],
                ),
                provider.clone(),
            ))
            .unwrap();
        runtime
            .block_on(router.register(
                "symbols",
                ResourceRoute::new(
                    "mem:///**",
                    Some("symbols/**"),
                    [ResourceOperation::Read, ResourceOperation::Children],
                ),
                provider,
            ))
            .unwrap();
        let model = ResourceMountModel::new(
            router,
            runtime.handle().clone(),
            PathBuf::from("/not-the-resource-tree"),
        );

        assert_eq!(
            model.entries(&MountNode::Root),
            [MountEntry {
                name: "mem".into(),
                node: MountNode::Scheme("mem".into()),
                directory: true,
                size: 0,
            }]
        );
        let scheme_entries = model.entries(&MountNode::Scheme("mem".into()));
        assert_eq!(scheme_entries[0].name, "dir");
        assert!(scheme_entries[0].directory);
        let directory_entries = model.entries(&scheme_entries[0].node);
        assert_eq!(
            directory_entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["doc", "doc~symbols"]
        );
        assert_eq!(
            model.child_uri(&scheme_entries[0].node, "doc~symbols"),
            Some(test_uri("mem:///dir/doc?symbols"))
        );
        assert!(matches!(
            model.handle(ResourceRequest::Write {
                uri: test_uri("mem:///dir/new"),
                text: "new".into(),
            }),
            Ok(ResourceReply::Written)
        ));
        assert!(matches!(
            model.handle(ResourceRequest::Move {
                from: test_uri("mem:///dir/doc"),
                to: Some(test_uri("mem:///dir/moved")),
            }),
            Ok(ResourceReply::Moved)
        ));
    }

    /// This one test body is compiled for, and can be run against, the active
    /// Linux FUSE, macFUSE, or WinFsp platform adapter.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires the native platform filesystem driver"]
    async fn native_platform_backend_conformance() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.rs");
        std::fs::write(&source, "fn symbol() {}\n").unwrap();

        let router = ResourceRouter::new();
        let filesystem: Arc<dyn ResourceProvider> = Arc::new(FilesystemProvider::new("/"));
        router
            .register(
                "filesystem",
                ResourceRoute::new(
                    "file:///**",
                    None::<String>,
                    [
                        ResourceOperation::Read,
                        ResourceOperation::Children,
                        ResourceOperation::Write,
                        ResourceOperation::Edit,
                        ResourceOperation::Move,
                    ],
                ),
                filesystem,
            )
            .await
            .unwrap();
        router
            .register(
                "symbols",
                ResourceRoute::new(
                    "file:///**/*.rs",
                    Some("symbols/**"),
                    [ResourceOperation::Read, ResourceOperation::Children],
                ),
                Arc::new(NativeSymbols),
            )
            .await
            .unwrap();

        let mount = super::platform::mount(router, Handle::current()).unwrap();
        let uri = ResourceUri::resolve(&source.to_string_lossy(), Path::new("/")).unwrap();
        assert_eq!(
            std::fs::read_to_string(uri_to_mount_path(mount.root(), &uri).unwrap()).unwrap(),
            "fn symbol() {}\n"
        );
        let projected = uri.descend_projection("symbols").unwrap();
        assert_eq!(
            std::fs::read_dir(uri_to_mount_path(mount.root(), &projected).unwrap())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["symbol"]
        );

        let created = source.with_file_name("created.txt");
        let created_uri = ResourceUri::resolve(&created.to_string_lossy(), Path::new("/")).unwrap();
        let created_mount = uri_to_mount_path(mount.root(), &created_uri).unwrap();
        std::fs::write(&created_mount, "mounted write\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&created).unwrap(),
            "mounted write\n"
        );
        let moved = source.with_file_name("moved.txt");
        let moved_uri = ResourceUri::resolve(&moved.to_string_lossy(), Path::new("/")).unwrap();
        let moved_mount = uri_to_mount_path(mount.root(), &moved_uri).unwrap();
        std::fs::rename(&created_mount, &moved_mount).unwrap();
        assert!(moved.exists());
        std::fs::remove_file(moved_mount).unwrap();
        assert!(!moved.exists());
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    /// macFUSE speaks the FUSE protocol used by `fuser`.
    pub struct MacFuseMount(crate::fuse::FuseMount);

    impl MountedResourceFilesystem for MacFuseMount {
        fn root(&self) -> &Path {
            self.0.root()
        }
    }

    pub fn mount(
        router: ResourceRouter,
        runtime: Handle,
    ) -> Result<Box<dyn MountedResourceFilesystem>, std::io::Error> {
        crate::fuse::FuseMount::mount(router, runtime)
            .map(MacFuseMount)
            .map(|mount| Box::new(mount) as Box<dyn MountedResourceFilesystem>)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;

    impl MountedResourceFilesystem for crate::winfsp::WinFspMount {
        fn root(&self) -> &Path {
            crate::winfsp::WinFspMount::root(self)
        }
    }

    pub fn mount(
        router: ResourceRouter,
        runtime: Handle,
    ) -> Result<Box<dyn MountedResourceFilesystem>, std::io::Error> {
        crate::winfsp::WinFspMount::mount(router, runtime)
            .map(|mount| Box::new(mount) as Box<dyn MountedResourceFilesystem>)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    use super::*;

    pub fn mount(
        _router: ResourceRouter,
        _runtime: Handle,
    ) -> Result<Box<dyn MountedResourceFilesystem>, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Artist has no mount adapter for this platform",
        ))
    }
}
