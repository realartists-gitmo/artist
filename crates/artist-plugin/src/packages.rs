use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{Arc, RwLock, Weak},
};

use artist_resource::{
    FilesystemProvider, ResourceError, ResourceReply, ResourceRequest, ResourceUri,
    TextReplacement, sha256,
};
use async_trait::async_trait;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PACKAGE_FORMAT: u32 = 1;
pub const MANIFEST_FILE: &str = "plugin.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPackageManifest {
    pub format: u32,
    pub id: String,
    pub cargo_package: String,
    pub component: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageState {
    #[default]
    Source,
    Built,
    Active,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackageStatus {
    pub state: PackageState,
    pub source_revision: Option<String>,
    pub candidate_revision: Option<String>,
    pub candidate_source_revision: Option<String>,
    pub active_revision: Option<String>,
    pub active_source_revision: Option<String>,
    pub diagnostics: String,
}

#[async_trait]
pub trait PluginActivator: Send + Sync {
    async fn activate(
        &self,
        package: &str,
        manifest: &PluginPackageManifest,
        component: &Path,
    ) -> Result<(), String>;
}

/// Host-owned mechanics for the canonical `plugins:///` package tree.
///
/// The package directory is the deployment unit: source, Cargo inputs and the
/// package manifest are inspectable and editable. Generated candidates and
/// status live under `.artist/`; activation never mutates the running plugin
/// until a complete candidate has instantiated and validated.
pub struct PluginPackages {
    root: PathBuf,
    filesystem: FilesystemProvider,
    lifecycle: tokio::sync::Mutex<()>,
    activator: RwLock<Option<Weak<dyn PluginActivator>>>,
}

impl PluginPackages {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            filesystem: FilesystemProvider::new(&root),
            root,
            lifecycle: tokio::sync::Mutex::new(()),
            activator: RwLock::new(None),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn set_activator(&self, activator: &Arc<dyn PluginActivator>) {
        *self
            .activator
            .write()
            .expect("plugin activator lock poisoned") = Some(Arc::downgrade(activator));
    }

    pub fn package_names(&self) -> Result<Vec<String>, ResourceError> {
        let mut names = std::fs::read_dir(&self.root)
            .map_err(provider)?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter(|entry| entry.path().join(MANIFEST_FILE).is_file())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    }

    pub fn manifest(&self, package: &str) -> Result<PluginPackageManifest, ResourceError> {
        validate_segment(package)?;
        let text = std::fs::read_to_string(self.root.join(package).join(MANIFEST_FILE))
            .map_err(provider)?;
        let manifest = serde_json::from_str::<PluginPackageManifest>(&text)
            .map_err(|error| ResourceError::Invalid(format!("invalid {MANIFEST_FILE}: {error}")))?;
        if manifest.format != PACKAGE_FORMAT {
            return Err(ResourceError::Invalid(format!(
                "unsupported plugin package format {}",
                manifest.format
            )));
        }
        if manifest.id.trim().is_empty()
            || manifest.cargo_package.trim().is_empty()
            || validate_segment(&manifest.component).is_err()
            || !manifest.component.ends_with(".wasm")
        {
            return Err(ResourceError::Invalid(
                "plugin manifest identity, Cargo package, and component must be non-empty; component must be one file name"
                    .into(),
            ));
        }
        Ok(manifest)
    }

    pub fn active_component(&self, package: &str) -> Result<PathBuf, ResourceError> {
        self.manifest(package)?;
        let path = self.generated(package)?.join("active.wasm");
        path.is_file().then_some(path).ok_or_else(|| {
            ResourceError::Invalid(format!(
                "plugin package `{package}` has no active component"
            ))
        })
    }

    pub async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
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
                let catalog_root = uri.as_url().path() == "/";
                let path = self.path(&uri)?;
                let mut directory = tokio::fs::read_dir(path).await.map_err(provider)?;
                let mut children = Vec::new();
                while let Some(entry) = directory.next_entry().await.map_err(provider)? {
                    let name = entry.file_name().into_string().map_err(|_| {
                        ResourceError::Provider("plugin package path is not UTF-8".into())
                    })?;
                    if name == "target" || entry.path().extension().is_some_and(|ext| ext == "wasm")
                    {
                        continue;
                    }
                    children.push(self.descend(&uri, &name)?);
                }
                if catalog_root
                    && self
                        .root
                        .parent()
                        .is_some_and(|parent| parent.join("wit").is_dir())
                {
                    children.push(self.descend(&uri, "_abi")?);
                }
                children.sort();
                Ok(ResourceReply::Children { children })
            }
            ResourceRequest::Write { uri, text } => {
                let package = self.package_name(&uri)?;
                let path = self.mutable_path(&uri)?;
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(provider)?;
                }
                tokio::fs::write(path, text).await.map_err(provider)?;
                self.mark_source_changed(&package)?;
                Ok(ResourceReply::Written)
            }
            ResourceRequest::Edit {
                uri,
                expected_sha256,
                replacements,
            } => self.edit(uri, expected_sha256, replacements).await,
            ResourceRequest::Move { from, to } => {
                let from_package = self.package_name(&from)?;
                let to_package = to.as_ref().map(|uri| self.package_name(uri)).transpose()?;
                let from = self.mutable_path(&from)?;
                match to {
                    Some(to) => {
                        let to = self.mutable_path(&to)?;
                        if let Some(parent) = to.parent() {
                            tokio::fs::create_dir_all(parent).await.map_err(provider)?;
                        }
                        tokio::fs::rename(from, to).await.map_err(provider)?;
                    }
                    None => {
                        let metadata = tokio::fs::metadata(&from).await.map_err(provider)?;
                        if metadata.is_dir() {
                            tokio::fs::remove_dir_all(from).await.map_err(provider)?;
                        } else {
                            tokio::fs::remove_file(from).await.map_err(provider)?;
                        }
                    }
                }
                self.mark_source_changed_if_present(&from_package)?;
                if let Some(package) = to_package
                    && package != from_package
                {
                    self.mark_source_changed_if_present(&package)?;
                }
                Ok(ResourceReply::Moved)
            }
            ResourceRequest::Signal { uri, name, payload } => {
                if payload.is_some() {
                    return Err(ResourceError::Invalid(
                        "plugin package lifecycle signals do not accept payloads".into(),
                    ));
                }
                let package = self.package_from_uri(&uri)?;
                match name.as_str() {
                    "build" => {
                        self.build(&package).await?;
                    }
                    "activate" => {
                        self.activate(&package).await?;
                    }
                    "build-and-activate" => {
                        self.build_and_activate(&package).await?;
                    }
                    _ => {
                        return Err(ResourceError::Invalid(format!(
                            "unknown plugin package signal `{name}`"
                        )));
                    }
                }
                Ok(ResourceReply::Signaled)
            }
            other => Err(ResourceError::Unsupported {
                uri: other.uri().clone(),
                operation: other.operation(),
            }),
        }
    }

    pub async fn build(&self, package: &str) -> Result<PathBuf, ResourceError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.build_unlocked(package).await
    }

    async fn build_unlocked(&self, package: &str) -> Result<PathBuf, ResourceError> {
        let manifest = self.manifest(package)?;
        let root = self.root.clone();
        let cargo_package = manifest.cargo_package.clone();
        let component = manifest.component.clone();
        let package_dir = self.root.join(package);
        let built_from_revision = source_revision(&self.root, &package_dir)?;
        let output = tokio::task::spawn_blocking(move || {
            Command::new("cargo")
                .current_dir(&root)
                .arg("test")
                .arg("--manifest-path")
                .arg(root.join("Cargo.toml"))
                .arg("-p")
                .arg(&cargo_package)
                .arg("--target-dir")
                .arg(root.join("target"))
                .output()
                .and_then(|tests| {
                    if !tests.status.success() {
                        return Ok((tests, None));
                    }
                    Command::new("cargo")
                        .current_dir(&root)
                        .arg("build")
                        .arg("--manifest-path")
                        .arg(root.join("Cargo.toml"))
                        .arg("-p")
                        .arg(cargo_package)
                        .arg("--target")
                        .arg("wasm32-wasip2")
                        .arg("--target-dir")
                        .arg(root.join("target"))
                        .output()
                        .map(|build| (tests, Some(build)))
                })
        })
        .await
        .map_err(|error| ResourceError::Provider(error.to_string()))?
        .map_err(provider)?;
        let mut diagnostics = command_output(&output.0);
        let success = output.0.status.success()
            && output.1.as_ref().is_some_and(|build| {
                diagnostics.push_str(&command_output(build));
                build.status.success()
            });
        if !success {
            self.write_status(
                package,
                PackageStatus {
                    state: PackageState::Failed,
                    source_revision: Some(built_from_revision),
                    diagnostics,
                    ..self.status(package)
                },
            )?;
            return Err(ResourceError::Provider(format!(
                "plugin package `{package}` did not build; inspect plugins:///{package}/.artist/status.json"
            )));
        }
        let current_source_revision = source_revision(&self.root, &package_dir)?;
        if current_source_revision != built_from_revision {
            diagnostics.push_str("source changed while the candidate was building\n");
            self.write_status(
                package,
                PackageStatus {
                    state: PackageState::Failed,
                    source_revision: Some(current_source_revision.clone()),
                    diagnostics,
                    ..self.status(package)
                },
            )?;
            return Err(ResourceError::Conflict {
                uri: ResourceUri::resolve(&format!("plugins:///{package}"), Path::new("/"))
                    .map_err(|error| ResourceError::Provider(error.to_string()))?,
                current_revision: current_source_revision,
            });
        }
        self.stage_artifact(package, &component, built_from_revision, diagnostics)
    }

    pub async fn activate(&self, package: &str) -> Result<PathBuf, ResourceError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.activate_unlocked(package).await
    }

    async fn build_and_activate(&self, package: &str) -> Result<PathBuf, ResourceError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.build_unlocked(package).await?;
        self.activate_unlocked(package).await
    }

    async fn activate_unlocked(&self, package: &str) -> Result<PathBuf, ResourceError> {
        let manifest = self.manifest(package)?;
        let candidate = self.generated(package)?.join("candidate.wasm");
        if !candidate.is_file() {
            return Err(ResourceError::Invalid(format!(
                "plugin package `{package}` has no built candidate"
            )));
        }
        let mut status = self.status(package);
        let current_source_revision = source_revision(&self.root, &self.root.join(package))?;
        if status.candidate_source_revision.as_deref() != Some(&current_source_revision) {
            status.state = PackageState::Source;
            status.source_revision = Some(current_source_revision.clone());
            status.diagnostics.push_str(
                "candidate was not activated because package source changed after its build\n",
            );
            self.write_status(package, status)?;
            return Err(ResourceError::Conflict {
                uri: ResourceUri::resolve(&format!("plugins:///{package}"), Path::new("/"))
                    .map_err(|error| ResourceError::Provider(error.to_string()))?,
                current_revision: current_source_revision,
            });
        }
        let activator = self
            .activator
            .read()
            .expect("plugin activator lock poisoned")
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| ResourceError::Provider("plugin activator is unavailable".into()))?;
        if let Err(error) = activator.activate(package, &manifest, &candidate).await {
            let mut status = self.status(package);
            status.state = PackageState::Failed;
            status
                .diagnostics
                .push_str(&format!("\nactivation failed: {error}\n"));
            self.write_status(package, status)?;
            return Err(ResourceError::Provider(error));
        }
        let active = self.generated(package)?.join("active.wasm");
        std::fs::copy(&candidate, &active).map_err(provider)?;
        status.state = PackageState::Active;
        status.active_revision = status.candidate_revision.clone();
        status.active_source_revision = status.candidate_source_revision.clone();
        self.write_status(package, status)?;
        Ok(active)
    }

    fn status(&self, package: &str) -> PackageStatus {
        std::fs::read_to_string(self.root.join(package).join(".artist/status.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn stage_artifact(
        &self,
        package: &str,
        component: &str,
        source_revision: String,
        diagnostics: String,
    ) -> Result<PathBuf, ResourceError> {
        let artifact = self.root.join("target/wasm32-wasip2/debug").join(component);
        let candidate = self.generated(package)?.join("candidate.wasm");
        std::fs::copy(&artifact, &candidate).map_err(provider)?;
        let candidate_revision = sha256(&std::fs::read(&candidate).map_err(provider)?);
        let previous = self.status(package);
        self.write_status(
            package,
            PackageStatus {
                state: PackageState::Built,
                source_revision: Some(source_revision.clone()),
                candidate_revision: Some(candidate_revision),
                candidate_source_revision: Some(source_revision),
                active_revision: previous.active_revision,
                active_source_revision: previous.active_source_revision,
                diagnostics,
            },
        )?;
        Ok(candidate)
    }

    fn write_status(&self, package: &str, status: PackageStatus) -> Result<(), ResourceError> {
        let generated = self.generated(package)?;
        let text = serde_json::to_string_pretty(&status)
            .map_err(|error| ResourceError::Provider(error.to_string()))?;
        std::fs::write(generated.join("status.json"), format!("{text}\n")).map_err(provider)
    }

    fn generated(&self, package: &str) -> Result<PathBuf, ResourceError> {
        validate_segment(package)?;
        let path = self.root.join(package).join(".artist");
        std::fs::create_dir_all(&path).map_err(provider)?;
        Ok(path)
    }

    fn package_name(&self, uri: &ResourceUri) -> Result<String, ResourceError> {
        uri_segments(uri)?
            .into_iter()
            .next()
            .ok_or_else(|| ResourceError::Invalid("the catalog root is not mutable".into()))
    }

    fn mark_source_changed(&self, package: &str) -> Result<(), ResourceError> {
        let mut status = self.status(package);
        status.state = PackageState::Source;
        status.source_revision = Some(source_revision(&self.root, &self.root.join(package))?);
        status.diagnostics =
            "package source changed; build a new candidate before activation\n".into();
        self.write_status(package, status)
    }

    fn mark_source_changed_if_present(&self, package: &str) -> Result<(), ResourceError> {
        self.root
            .join(package)
            .is_dir()
            .then(|| self.mark_source_changed(package))
            .transpose()
            .map(|_| ())
    }

    fn package_from_uri(&self, uri: &ResourceUri) -> Result<String, ResourceError> {
        let segments = uri_segments(uri)?;
        if segments.len() != 1 {
            return Err(ResourceError::Invalid(
                "plugin lifecycle signals target a package root".into(),
            ));
        }
        self.manifest(&segments[0])?;
        Ok(segments[0].clone())
    }

    fn path(&self, uri: &ResourceUri) -> Result<PathBuf, ResourceError> {
        let segments = uri_segments(uri)?;
        let abi = segments.first().is_some_and(|segment| segment == "_abi");
        let mut path = if abi {
            self.root
                .parent()
                .ok_or_else(|| ResourceError::Provider("plugin catalog has no parent".into()))?
                .join("wit")
        } else {
            self.root.clone()
        };
        for segment in segments.into_iter().skip(usize::from(abi)) {
            path.push(segment);
        }
        Ok(path)
    }

    fn mutable_path(&self, uri: &ResourceUri) -> Result<PathBuf, ResourceError> {
        let segments = uri_segments(uri)?;
        if segments
            .first()
            .is_none_or(|package| self.manifest(package).is_err())
            || segments.iter().any(|segment| segment == ".artist")
        {
            return Err(ResourceError::Invalid(
                "only files inside an existing plugin package may be modified; generated .artist state is host-owned"
                    .into(),
            ));
        }
        let mut path = self.root.clone();
        for segment in segments {
            path.push(segment);
        }
        Ok(path)
    }

    fn descend(&self, parent: &ResourceUri, segment: &str) -> Result<ResourceUri, ResourceError> {
        validate_segment(segment)?;
        let mut url = parent.as_url().clone();
        url.path_segments_mut()
            .map_err(|_| ResourceError::Invalid("plugins URI cannot be a base".into()))?
            .pop_if_empty()
            .push(segment);
        ResourceUri::from_url(url).map_err(|error| ResourceError::Invalid(error.to_string()))
    }

    async fn edit(
        &self,
        uri: ResourceUri,
        expected: String,
        replacements: Vec<TextReplacement>,
    ) -> Result<ResourceReply, ResourceError> {
        let path = self.mutable_path(&uri)?;
        let file_uri = ResourceUri::resolve(&path.to_string_lossy(), Path::new("/"))
            .map_err(|error| ResourceError::Provider(error.to_string()))?;
        self.filesystem
            .edit(file_uri, expected, replacements)
            .await
            .map_err(|error| match error {
                ResourceError::Conflict {
                    current_revision, ..
                } => ResourceError::Conflict {
                    uri: uri.clone(),
                    current_revision,
                },
                other => other,
            })?;
        self.mark_source_changed(&self.package_name(&uri)?)?;
        Ok(ResourceReply::Edited {
            revision: sha256(&std::fs::read(path).map_err(provider)?),
        })
    }
}

fn uri_segments(uri: &ResourceUri) -> Result<Vec<String>, ResourceError> {
    let url = uri.as_url();
    if url.scheme() != "plugins" || url.host_str().is_some() || url.query().is_some() {
        return Err(ResourceError::Invalid(format!(
            "expected a plugins:/// URI, got {uri}"
        )));
    }
    url.path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|encoded| {
            let segment = percent_decode_str(encoded)
                .decode_utf8()
                .map_err(|_| ResourceError::Invalid("plugin path is not UTF-8".into()))?;
            validate_segment(&segment)?;
            Ok(segment.into_owned())
        })
        .collect()
}

fn validate_segment(segment: &str) -> Result<(), ResourceError> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.contains(['/', '\\'])
        || Path::new(segment)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ResourceError::Invalid("invalid plugin path segment".into()));
    }
    Ok(())
}

fn source_revision(catalog: &Path, package: &Path) -> Result<String, ResourceError> {
    fn visit(
        label: &Path,
        root: &Path,
        path: &Path,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> std::io::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        if path.is_file() {
            files.insert(label.to_owned(), std::fs::read(path)?);
            return Ok(());
        }
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == ".artist" || name == "target" {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                visit(label, root, &path, files)?;
            } else {
                files.insert(
                    label.join(path.strip_prefix(root).unwrap()),
                    std::fs::read(path)?,
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(Path::new("package"), package, package, &mut files).map_err(provider)?;
    for name in ["Cargo.toml", "Cargo.lock"] {
        visit(
            Path::new(name),
            &catalog.join(name),
            &catalog.join(name),
            &mut files,
        )
        .map_err(provider)?;
    }
    visit(
        Path::new("sdk"),
        &catalog.join("sdk"),
        &catalog.join("sdk"),
        &mut files,
    )
    .map_err(provider)?;
    if let Some(parent) = catalog.parent() {
        visit(
            Path::new("wit"),
            &parent.join("wit"),
            &parent.join("wit"),
            &mut files,
        )
        .map_err(provider)?;
    }
    let mut digest = Sha256::new();
    for (path, bytes) in files {
        digest.update(path.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn command_output(output: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn slice_lines(text: &str, start_line: Option<u64>, line_count: Option<u64>) -> String {
    let start = start_line.unwrap_or(1).saturating_sub(1) as usize;
    let count = line_count.map_or(usize::MAX, |count| count as usize);
    text.lines()
        .skip(start)
        .take(count)
        .collect::<Vec<_>>()
        .join("\n")
}

fn provider(error: impl std::fmt::Display) -> ResourceError {
    ResourceError::Provider(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct RejectActivation;
    struct AcceptActivation(AtomicBool);

    #[async_trait]
    impl PluginActivator for RejectActivation {
        async fn activate(
            &self,
            _: &str,
            _: &PluginPackageManifest,
            _: &Path,
        ) -> Result<(), String> {
            Err("candidate rejected".into())
        }
    }

    #[async_trait]
    impl PluginActivator for AcceptActivation {
        async fn activate(
            &self,
            _: &str,
            _: &PluginPackageManifest,
            _: &Path,
        ) -> Result<(), String> {
            self.0.store(true, Ordering::Release);
            Ok(())
        }
    }

    fn uri(text: &str) -> ResourceUri {
        ResourceUri::resolve(text, Path::new("/")).unwrap()
    }

    #[tokio::test]
    async fn package_sources_are_inspectable_editable_and_generated_state_is_protected() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("read");
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(
            package.join(MANIFEST_FILE),
            r#"{"format":1,"id":"artist.tool.read","cargo_package":"artist-tool-read","component":"artist_tool_read.wasm"}"#,
        )
        .unwrap();
        std::fs::write(package.join("src/lib.rs"), "old\n").unwrap();
        let store = PluginPackages::new(temp.path());

        let children = store
            .handle(ResourceRequest::Children {
                uri: uri("plugins:///"),
            })
            .await
            .unwrap();
        assert!(
            matches!(children, ResourceReply::Children { children } if children == [uri("plugins:///read")])
        );
        store
            .handle(ResourceRequest::Write {
                uri: uri("plugins:///read/src/lib.rs"),
                text: "new\n".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(package.join("src/lib.rs")).unwrap(),
            "new\n"
        );
        assert!(
            store
                .handle(ResourceRequest::Write {
                    uri: uri("plugins:///read/.artist/status.json"),
                    text: "tamper".into(),
                })
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejected_activation_preserves_the_active_artifact_and_records_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("read");
        std::fs::create_dir_all(package.join(".artist")).unwrap();
        std::fs::write(
            package.join(MANIFEST_FILE),
            r#"{"format":1,"id":"artist.tool.read","cargo_package":"artist-tool-read","component":"artist_tool_read.wasm"}"#,
        )
        .unwrap();
        std::fs::write(package.join(".artist/candidate.wasm"), b"candidate").unwrap();
        std::fs::write(package.join(".artist/active.wasm"), b"active").unwrap();
        let store = PluginPackages::new(temp.path());
        let revision = source_revision(temp.path(), &package).unwrap();
        store
            .write_status(
                "read",
                PackageStatus {
                    state: PackageState::Built,
                    source_revision: Some(revision.clone()),
                    candidate_source_revision: Some(revision),
                    ..PackageStatus::default()
                },
            )
            .unwrap();
        let activator: Arc<dyn PluginActivator> = Arc::new(RejectActivation);
        store.set_activator(&activator);

        assert!(store.activate("read").await.is_err());
        assert_eq!(
            std::fs::read(package.join(".artist/active.wasm")).unwrap(),
            b"active"
        );
        let status = store.status("read");
        assert_eq!(status.state, PackageState::Failed);
        assert!(status.diagnostics.contains("candidate rejected"));
    }

    #[tokio::test]
    async fn source_changes_make_a_built_candidate_ineligible_for_activation() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("read");
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::create_dir_all(package.join(".artist")).unwrap();
        std::fs::write(
            package.join(MANIFEST_FILE),
            r#"{"format":1,"id":"artist.tool.read","cargo_package":"artist-tool-read","component":"artist_tool_read.wasm"}"#,
        )
        .unwrap();
        std::fs::write(package.join("src/lib.rs"), "before\n").unwrap();
        std::fs::write(package.join(".artist/candidate.wasm"), b"candidate").unwrap();
        let store = PluginPackages::new(temp.path());
        let revision = source_revision(temp.path(), &package).unwrap();
        store
            .write_status(
                "read",
                PackageStatus {
                    state: PackageState::Built,
                    source_revision: Some(revision.clone()),
                    candidate_source_revision: Some(revision),
                    ..PackageStatus::default()
                },
            )
            .unwrap();
        let activator = Arc::new(AcceptActivation(AtomicBool::new(false)));
        let erased: Arc<dyn PluginActivator> = activator.clone();
        store.set_activator(&erased);
        store
            .handle(ResourceRequest::Write {
                uri: uri("plugins:///read/src/lib.rs"),
                text: "after\n".into(),
            })
            .await
            .unwrap();

        assert!(matches!(
            store.activate("read").await,
            Err(ResourceError::Conflict { .. })
        ));
        assert!(!activator.0.load(Ordering::Acquire));
    }

    #[test]
    fn stale_package_formats_are_rejected_without_a_compatibility_path() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("read");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join(MANIFEST_FILE),
            r#"{"format":0,"id":"artist.tool.read","cargo_package":"artist-tool-read","component":"artist_tool_read.wasm"}"#,
        )
        .unwrap();
        assert!(matches!(
            PluginPackages::new(temp.path()).manifest("read"),
            Err(ResourceError::Invalid(message)) if message.contains("unsupported plugin package format")
        ));
    }
}
