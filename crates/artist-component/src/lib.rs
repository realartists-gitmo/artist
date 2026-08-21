//! Component artifact validation and activation preparation.
//!
//! Source builds belong behind a separate build service. This crate deals with
//! the stable artifact boundary consumed by the runtime.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use artist_kernel::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use artist_kernel::{Kernel, ResourceUri};
use artist_wasm::{
    Extension, ExtensionClass, ExtensionDependency, ExtensionMetadata, PreparedGeneration, Runtime,
};
use artist_wasm_nouns::{RoutedNoun, WasmRoutedNoun};
use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use wasmtime::Engine;

pub mod agents;
pub mod bootstrap;
pub mod compaction;
pub mod composition;
pub mod composition_extension;
pub mod harness;
pub mod identity;
pub mod policy;
pub mod process;
pub mod profile;
pub mod providers;
pub mod tools;

pub use agents::{
    AgentProcess, AgentResourceComponent, AgentResourceSocket, AgentTranscript, EventLogTranscript,
};
pub use artist_wasm_composition::types::SessionInput as CompositionInput;
pub use bootstrap::ComponentHost;
pub use compaction::{
    CompactionComponent, CompactionError, CompactionRequest, CompactionResponse, CompactionSocket,
    WasmCompactionComponent,
};
pub use composition::{
    CompositionWatcher, PackageComponentLoader, UrlComposition, UrlCompositionSource,
};
pub use composition_extension::{CompositionUpdate, WasmComposition};
pub use harness::{HarnessError, HarnessOperation, HarnessPolicy, HarnessSocket};
pub use identity::IdentityCatalog;
pub use policy::{
    DirectoryResourceProvider, PermissionEffect, PermissionRegistry, PermissionRule,
    install_profile_view, install_prompt_view,
};
pub use process::{ProcessRegistry, ProcessSocket};
pub use profile::{FileProfileComponent, ProfileComponent, ProfileDocument, ProfileSocket};
pub use providers::{
    DefaultProviderConfigurationComponent, ProviderConfigurationComponent, ProviderSocket,
};
pub use tools::{
    ComponentToolRegistry, ToolComponent, ToolError, ToolFailure, ToolResultEnvelope,
    WasmToolComponent,
};

/// Session-bound composition source. The agent loop calls this at each model
/// boundary and applies the returned provider-neutral events to its
/// model-facing surface.
#[async_trait::async_trait]
pub trait CompositionUpdater: Send + Sync {
    async fn update(&self, input: CompositionInput) -> anyhow::Result<Vec<CompositionUpdate>>;
}

const PACKAGE_MANIFEST: &str = "extension.toml";
const PACKAGE_ARTIFACT: &str = "extension.wasm";
const PACKAGE_SOURCE: &str = "src";
const PACKAGE_README: &str = "README.md";

/// The on-disk contract for an Artist extension package.
///
/// The executable, source, and documentation are deliberately fixed package
/// members. The manifest carries the metadata that the low-level runtime
/// otherwise receives as a separate argument.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ExtensionManifest {
    pub name: String,
    pub version: String,
    /// Optional declarations are checked against the component's actual
    /// exported Artist contract families. Omitting the field preserves the
    /// package format for older manifests while never allowing a false claim.
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub route_hints: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<ManifestDependency>,
    /// Model-facing contracts supplied by this component package. A bare
    /// route hint for a verb must have a matching declaration here.
    #[serde(default)]
    pub tools: Vec<ToolManifest>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ManifestDependency {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ToolManifest {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema encoded as a string so the package manifest remains a
    /// simple TOML contract while the component layer validates it once.
    pub input_schema: String,
}

impl ExtensionManifest {
    fn metadata(&self) -> ArtifactMetadata {
        ArtifactMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
            route_hints: self.route_hints.clone(),
            dependencies: self
                .dependencies
                .iter()
                .map(|dependency| ExtensionDependency {
                    name: dependency.name.clone(),
                    version: dependency.version.clone(),
                })
                .collect(),
        }
    }

    fn validate_roles(&self, class: ExtensionClass) -> anyhow::Result<()> {
        for role in &self.roles {
            let present = match role.as_str() {
                "noun" => class.noun,
                "verb" => class.verb,
                "event" => class.event,
                "composition" => class.composition,
                other => return Err(anyhow!("unknown extension role {other:?}")),
            };
            if !present {
                return Err(anyhow!(
                    "extension declares role {role:?} but its WIT exports do not provide it"
                ));
            }
        }
        Ok(())
    }
}

/// A validated extension package. The wasm artifact is the executable
/// authority; source and documentation are package contents for inspection,
/// rebuilding, and presentation.
pub struct ExtensionPackage {
    root: PathBuf,
    manifest: ExtensionManifest,
    artifact: ComponentArtifact,
}

/// Component-layer catalog of installed extension packages.
///
/// This is deliberately not a kernel namespace. The catalog is an ordinary
/// provider which an extension host may install at `resource://`; the kernel
/// owns only URI routing and the `url://` namespace-registration root.
#[derive(Clone, Default)]
pub struct ExtensionCatalog {
    packages: Arc<RwLock<BTreeMap<String, PathBuf>>>,
    kernel: Arc<RwLock<Option<Arc<Kernel>>>>,
}

impl ExtensionCatalog {
    pub fn install_into(&self, kernel: &Arc<Kernel>) {
        *self.kernel.write().unwrap() = Some(Arc::clone(kernel));
        kernel.register_resource_provider(self.clone());
    }

    pub fn publish(&self, package: &ExtensionPackage) {
        self.packages
            .write()
            .unwrap()
            .insert(package.manifest.name.clone(), package.root.clone());
    }

    /// Publish and activate one package through the existing runtime lifecycle.
    /// The package remains discoverable at `resource://<name>` for as long as
    /// it remains published; retiring the generation is controlled by Runtime.
    pub async fn activate(
        &self,
        package: &ExtensionPackage,
        runtime: &Runtime,
    ) -> anyhow::Result<artist_wasm::GenerationHandle> {
        let prepared = package.prepare(runtime)?;
        let handle = runtime.activate(prepared).await?;
        self.publish(package);
        if handle.class().noun {
            let kernel = self
                .kernel
                .read()
                .unwrap()
                .clone()
                .ok_or_else(|| anyhow!("extension catalog is not installed into a kernel"))?;
            kernel.register_resource_provider(RoutedNoun::new(
                format!("noun:{}", handle.name()),
                Arc::new(WasmRoutedNoun::new(handle.clone())),
            ));
        }
        Ok(handle)
    }

    pub fn unpublish(&self, name: &str) -> bool {
        self.packages.write().unwrap().remove(name).is_some()
    }

    fn target(&self, uri: &ResourceUri) -> Result<Option<PathBuf>, ResourceError> {
        if uri.scheme() != "resource" {
            return Err(ResourceError::not_found(uri));
        }
        let mut segments = uri.decoded_segments().map_err(|_| {
            ResourceError::new(
                ResourceErrorCode::InvalidAddress,
                format!("invalid URI: {uri}"),
            )
        })?;
        let name = if uri.authority().is_empty() {
            let Some(name) = segments.first().cloned() else {
                return Ok(None);
            };
            segments.remove(0);
            name
        } else {
            uri.authority().to_string()
        };
        let Some(root) = self.packages.read().unwrap().get(&name).cloned() else {
            return Ok(None);
        };
        let mut path = root;
        for segment in segments {
            path.push(segment);
        }
        Ok(Some(path))
    }

    fn io_error(uri: &ResourceUri, error: std::io::Error) -> ResourceError {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ResourceErrorCode::NotFound,
            std::io::ErrorKind::PermissionDenied => ResourceErrorCode::PermissionDenied,
            _ => ResourceErrorCode::Io,
        };
        ResourceError::new(
            code,
            format!("resource catalog access failed for {uri}: {error}"),
        )
    }
}

#[async_trait]
impl ResourceProvider for ExtensionCatalog {
    fn provider_name(&self) -> &str {
        "extension-resource-catalog"
    }

    fn eligible(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == "resource"
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        if !self.eligible(uri) {
            return Err(ResourceError::not_found(uri));
        }
        let Some(path) = self.target(uri)? else {
            return Ok(ProviderAttrs::directory());
        };
        let metadata = std::fs::metadata(&path).map_err(|error| Self::io_error(uri, error))?;
        if metadata.is_dir() {
            Ok(ProviderAttrs::directory())
        } else {
            Ok(ProviderAttrs::file(
                metadata.len(),
                metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            ))
        }
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        if !self.eligible(uri) {
            return Err(ResourceError::not_found(uri));
        }
        let Some(path) = self.target(uri)? else {
            return Ok(self
                .packages
                .read()
                .unwrap()
                .keys()
                .map(|name| ProviderEntry {
                    name: name.clone(),
                    attrs: ProviderAttrs::directory(),
                })
                .collect());
        };
        let mut output = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|error| Self::io_error(uri, error))? {
            let entry = entry.map_err(|error| Self::io_error(uri, error))?;
            let metadata = entry
                .metadata()
                .map_err(|error| Self::io_error(uri, error))?;
            let attrs = if metadata.is_dir() {
                ProviderAttrs::directory()
            } else {
                ProviderAttrs::file(
                    metadata.len(),
                    metadata
                        .modified()
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                )
            };
            output.push(ProviderEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                attrs,
            });
        }
        output.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(output)
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        const MAX_CATALOG_READ: u32 = 64 * 1024 * 1024;
        let Some(path) = self.target(uri)? else {
            return Err(ResourceError::new(
                ResourceErrorCode::IsDir,
                "cannot read resource catalog root",
            ));
        };
        let mut file = std::fs::File::open(&path).map_err(|error| Self::io_error(uri, error))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| Self::io_error(uri, error))?;
        let mut bytes = vec![0_u8; size.min(MAX_CATALOG_READ) as usize];
        let count = file
            .read(&mut bytes)
            .map_err(|error| Self::io_error(uri, error))?;
        bytes.truncate(count);
        Ok(bytes)
    }
}

impl ExtensionPackage {
    pub fn open(engine: &Engine, root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(anyhow!(
                "extension package is not a directory: {}",
                root.display()
            ));
        }

        let manifest_path = root.join(PACKAGE_MANIFEST);
        let manifest_text = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("read package manifest {}", manifest_path.display()))?;
        let manifest: ExtensionManifest = toml::from_str(&manifest_text)
            .with_context(|| format!("parse package manifest {}", manifest_path.display()))?;
        let metadata = manifest.metadata();
        metadata.validate()?;

        require_package_file(root, PACKAGE_ARTIFACT)?;
        require_package_file(root, PACKAGE_README)?;
        require_package_directory(root, PACKAGE_SOURCE)?;

        let artifact = ComponentArtifact::from_file(engine, root.join(PACKAGE_ARTIFACT))?;
        manifest.validate_roles(artifact.class())?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
            artifact,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    pub fn artifact(&self) -> &ComponentArtifact {
        &self.artifact
    }

    pub fn source_root(&self) -> PathBuf {
        self.root.join(PACKAGE_SOURCE)
    }

    pub fn tool_definitions(
        &self,
    ) -> anyhow::Result<BTreeMap<String, llm_provider::ToolDefinition>> {
        let mut definitions = BTreeMap::new();
        for tool in &self.manifest.tools {
            if tool.name.trim().is_empty() {
                return Err(anyhow!("tool definition name cannot be empty"));
            }
            let input_schema = serde_json::from_str(&tool.input_schema).with_context(|| {
                format!("parse input schema for component tool {:?}", tool.name)
            })?;
            if definitions
                .insert(
                    tool.name.clone(),
                    llm_provider::ToolDefinition {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        input_schema,
                    },
                )
                .is_some()
            {
                return Err(anyhow!(
                    "duplicate component tool definition {:?}",
                    tool.name
                ));
            }
        }
        Ok(definitions)
    }

    pub fn readme_path(&self) -> PathBuf {
        self.root.join(PACKAGE_README)
    }

    pub fn prepare(&self, runtime: &Runtime) -> anyhow::Result<PreparedGeneration> {
        self.artifact.prepare(runtime, self.manifest.metadata())
    }
}

fn require_package_file(root: &Path, name: &str) -> anyhow::Result<()> {
    let path = root.join(name);
    if !path.is_file() {
        return Err(anyhow!(
            "extension package requires file {}",
            path.display()
        ));
    }
    Ok(())
}

fn require_package_directory(root: &Path, name: &str) -> anyhow::Result<()> {
    let path = root.join(name);
    if !path.is_dir() {
        return Err(anyhow!(
            "extension package requires directory {}",
            path.display()
        ));
    }
    if path.read_dir()?.next().transpose()?.is_none() {
        return Err(anyhow!(
            "extension package source directory is empty: {}",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRequest {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
    pub artifact_path: PathBuf,
    pub metadata: ArtifactMetadata,
}

impl BuildRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.program.trim().is_empty() {
            return Err(anyhow!("build program cannot be empty"));
        }
        if self.artifact_path.as_os_str().is_empty() {
            return Err(anyhow!("artifact path cannot be empty"));
        }
        self.metadata.validate()
    }
}

pub struct BuildOutput {
    pub artifact: ComponentArtifact,
    pub metadata: ArtifactMetadata,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("invalid build request: {0}")]
    Invalid(#[from] anyhow::Error),
    #[error("build process failed with status {status:?}: {stderr}")]
    Failed {
        status: Option<i32>,
        stdout: String,
        stderr: String,
    },
    #[error("build process exceeded the configured timeout")]
    TimedOut,
    #[error("could not read build artifact {path}: {source}")]
    ArtifactIo {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("built artifact is invalid: {0}")]
    Artifact(#[source] anyhow::Error),
    #[error("build artifact exceeds the configured {limit} byte limit")]
    ArtifactTooLarge { limit: u64 },
}

#[async_trait::async_trait]
pub trait SourceBuildService: Send + Sync {
    async fn build(&self, request: BuildRequest) -> Result<BuildOutput, BuildError>;
}

/// Executes an external language/toolchain build and validates its component
/// output. The runtime never needs to know which language produced the bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildLimits {
    pub timeout: Duration,
    pub max_artifact_bytes: u64,
}

impl Default for BuildLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10 * 60),
            max_artifact_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone)]
pub struct CommandBuildService {
    engine: Engine,
    limits: BuildLimits,
    cache: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
}

impl CommandBuildService {
    pub fn new(engine: Engine) -> Self {
        Self::with_limits(engine, BuildLimits::default())
    }

    pub fn with_limits(engine: Engine, limits: BuildLimits) -> Self {
        Self {
            engine,
            limits,
            cache: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

#[async_trait::async_trait]
impl SourceBuildService for CommandBuildService {
    async fn build(&self, request: BuildRequest) -> Result<BuildOutput, BuildError> {
        request.validate().map_err(BuildError::Invalid)?;
        let cache_key = build_cache_key(&request).map_err(BuildError::Invalid)?;
        if let Some(bytes) = self.cache.lock().unwrap().get(&cache_key).cloned() {
            let artifact =
                ComponentArtifact::from_bytes(&self.engine, bytes).map_err(BuildError::Artifact)?;
            return Ok(BuildOutput {
                artifact,
                metadata: request.metadata,
                stdout: b"component build cache hit\n".to_vec(),
                stderr: Vec::new(),
            });
        }
        let output = tokio::time::timeout(
            self.limits.timeout,
            tokio::process::Command::new(&request.program)
                .args(&request.args)
                .current_dir(&request.current_dir)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| BuildError::TimedOut)?
        .map_err(|error| BuildError::Failed {
            status: None,
            stdout: String::new(),
            stderr: error.to_string(),
        })?;
        if !output.status.success() {
            return Err(BuildError::Failed {
                status: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let bytes = tokio::fs::read(&request.artifact_path)
            .await
            .map_err(|source| BuildError::ArtifactIo {
                path: request.artifact_path.clone(),
                source,
            })?;
        if bytes.len() as u64 > self.limits.max_artifact_bytes {
            return Err(BuildError::ArtifactTooLarge {
                limit: self.limits.max_artifact_bytes,
            });
        }
        let artifact =
            ComponentArtifact::from_bytes(&self.engine, bytes).map_err(BuildError::Artifact)?;
        self.cache
            .lock()
            .unwrap()
            .insert(cache_key, artifact.bytes().to_vec());
        Ok(BuildOutput {
            artifact,
            metadata: request.metadata,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

fn build_cache_key(request: &BuildRequest) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(request.program.as_bytes());
    for arg in &request.args {
        hasher.update([0]);
        hasher.update(arg.as_bytes());
    }
    hasher.update(request.current_dir.to_string_lossy().as_bytes());
    hasher.update(request.artifact_path.to_string_lossy().as_bytes());
    hasher.update(request.metadata.name.as_bytes());
    hasher.update(request.metadata.version.as_bytes());
    for route in &request.metadata.route_hints {
        hasher.update(route.as_bytes());
    }
    for dependency in &request.metadata.dependencies {
        hasher.update(dependency.name.as_bytes());
        hasher.update(dependency.version.as_bytes());
    }
    if request.current_dir.is_dir() {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(&request.current_dir) {
            let entry = entry?;
            if !entry.file_type().is_file() || entry.path() == request.artifact_path {
                continue;
            }
            files.push(entry.into_path());
        }
        files.sort();
        for path in files {
            hasher.update(
                path.strip_prefix(&request.current_dir)?
                    .to_string_lossy()
                    .as_bytes(),
            );
            hasher.update(std::fs::read(path)?);
        }
    }
    Ok(hex_digest(&hasher.finalize()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactMetadata {
    pub name: String,
    pub version: String,
    pub route_hints: Vec<String>,
    pub dependencies: Vec<ExtensionDependency>,
}

impl ArtifactMetadata {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.name.trim().is_empty() {
            return Err(anyhow!("component name cannot be empty"));
        }
        if self.version.trim().is_empty() {
            return Err(anyhow!("component version cannot be empty"));
        }
        ExtensionMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
            route_hints: self.route_hints.clone(),
            dependencies: self.dependencies.clone(),
        }
        .validate()?;
        Ok(())
    }

    fn runtime_metadata(&self) -> ExtensionMetadata {
        ExtensionMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
            route_hints: self.route_hints.clone(),
            dependencies: self.dependencies.clone(),
        }
    }
}

/// A validated, immutable component artifact.
pub struct ComponentArtifact {
    bytes: Vec<u8>,
    digest: String,
    class: ExtensionClass,
}

impl ComponentArtifact {
    pub fn from_bytes(engine: &Engine, bytes: impl Into<Vec<u8>>) -> anyhow::Result<Self> {
        let bytes = bytes.into();
        let extension = Extension::load(engine, &bytes).context("validate wasm component")?;
        if extension.class.is_empty() {
            return Err(anyhow!("component exports no Artist contract family"));
        }
        let digest = hex_digest(&bytes);
        Ok(Self {
            bytes,
            digest,
            class: extension.class,
        })
    }

    pub fn from_file(engine: &Engine, path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let bytes =
            std::fs::read(path).with_context(|| format!("read component {}", path.display()))?;
        Self::from_bytes(engine, bytes)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn class(&self) -> ExtensionClass {
        self.class
    }

    pub fn prepare(
        &self,
        runtime: &Runtime,
        metadata: ArtifactMetadata,
    ) -> anyhow::Result<PreparedGeneration> {
        metadata.validate()?;
        runtime.prepare(&self.bytes, metadata.runtime_metadata())
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_kernel::Kernel;
    use artist_wasm::{KernelHostEnvironment, build_engine};
    use std::sync::Arc;

    const NOUN_COMPONENT: &str = r#"(component
        (core module $m (func (export "f")))
        (export "artist:nouns/provider@2.0.0" (core module $m))
    )"#;

    #[test]
    fn validates_and_hashes_component_artifacts() {
        let engine = build_engine().unwrap();
        let artifact = ComponentArtifact::from_bytes(&engine, NOUN_COMPONENT.as_bytes()).unwrap();
        assert!(artifact.digest().len() == 64);
        assert!(artifact.class().noun);
        assert!(!artifact.class().verb);
    }

    #[test]
    fn opens_only_complete_extension_packages() {
        let engine = build_engine().unwrap();
        let package = tempfile::tempdir().unwrap();
        std::fs::write(
            package.path().join("extension.toml"),
            r#"
name = "demo"
version = "1.0.0"
route_hints = ["file://**/*.rs/symbols"]

[[dependencies]]
name = "base"
version = ">=1.0.0"
"#,
        )
        .unwrap();
        std::fs::write(
            package.path().join("extension.wasm"),
            NOUN_COMPONENT.as_bytes(),
        )
        .unwrap();
        std::fs::write(package.path().join("README.md"), "Demo extension").unwrap();
        std::fs::create_dir(package.path().join("src")).unwrap();
        std::fs::write(package.path().join("src/main.rs"), "fn main() {}").unwrap();

        let loaded = ExtensionPackage::open(&engine, package.path()).unwrap();
        assert_eq!(loaded.manifest().name, "demo");
        assert_eq!(loaded.manifest().dependencies[0].name, "base");
        assert_eq!(loaded.source_root(), package.path().join("src"));
        assert!(loaded.artifact().class().noun);
    }

    #[tokio::test]
    async fn publishes_packages_at_resource_uris_without_creating_a_namespace() {
        let engine = build_engine().unwrap();
        let package = tempfile::tempdir().unwrap();
        std::fs::write(
            package.path().join("extension.toml"),
            "name = 'demo'\nversion = '1.0.0'\n",
        )
        .unwrap();
        std::fs::write(
            package.path().join("extension.wasm"),
            NOUN_COMPONENT.as_bytes(),
        )
        .unwrap();
        std::fs::write(package.path().join("README.md"), "Demo extension").unwrap();
        std::fs::create_dir(package.path().join("src")).unwrap();
        std::fs::write(package.path().join("src/main.rs"), "fn main() {}").unwrap();

        let loaded = ExtensionPackage::open(&engine, package.path()).unwrap();
        let catalog = ExtensionCatalog::default();
        catalog.publish(&loaded);
        let kernel = Arc::new(Kernel::new());
        catalog.install_into(&kernel);
        let runtime = Runtime::new(
            engine,
            Arc::new(KernelHostEnvironment::new(Arc::clone(&kernel))),
        );
        let handle = catalog.activate(&loaded, &runtime).await.unwrap();
        assert_eq!(handle.name(), "demo");

        let uri: ResourceUri = "resource://demo/README.md".parse().unwrap();
        assert_eq!(
            kernel.read_uri(&uri, 0, 64).await.unwrap(),
            b"Demo extension"
        );
        assert_eq!(kernel.namespace_names(), vec!["url"]);
    }

    #[test]
    fn rejects_packages_missing_source_or_documentation() {
        let engine = build_engine().unwrap();
        let package = tempfile::tempdir().unwrap();
        std::fs::write(
            package.path().join("extension.toml"),
            "name = 'demo'\nversion = '1.0.0'\n",
        )
        .unwrap();
        std::fs::write(
            package.path().join("extension.wasm"),
            NOUN_COMPONENT.as_bytes(),
        )
        .unwrap();
        std::fs::create_dir(package.path().join("src")).unwrap();
        assert!(ExtensionPackage::open(&engine, package.path()).is_err());
    }

    #[tokio::test]
    async fn prepares_artifact_for_runtime_activation() {
        let engine = build_engine().unwrap();
        let artifact = ComponentArtifact::from_bytes(&engine, NOUN_COMPONENT.as_bytes()).unwrap();
        let runtime = Runtime::new(
            engine,
            Arc::new(KernelHostEnvironment::new(Arc::new(Kernel::empty()))),
        );
        let prepared = artifact
            .prepare(
                &runtime,
                ArtifactMetadata {
                    name: "demo".into(),
                    version: "1.0.0".into(),
                    route_hints: vec!["file://**/*.rs/symbols".into()],
                    dependencies: Vec::new(),
                },
            )
            .unwrap();
        let handle = runtime.activate(prepared).await.unwrap();
        assert_eq!(handle.name(), "demo");
    }

    #[test]
    fn rejects_invalid_metadata_before_runtime_prepare() {
        let engine = build_engine().unwrap();
        let artifact = ComponentArtifact::from_bytes(&engine, NOUN_COMPONENT.as_bytes()).unwrap();
        let runtime = Runtime::new(
            engine,
            Arc::new(KernelHostEnvironment::new(Arc::new(Kernel::empty()))),
        );
        assert!(
            artifact
                .prepare(
                    &runtime,
                    ArtifactMetadata {
                        name: String::new(),
                        version: "1.0.0".into(),
                        route_hints: vec!["file://**/*.rs/symbols".into()],
                        dependencies: Vec::new(),
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn build_requests_validate_without_assuming_a_language() {
        let request = BuildRequest {
            program: "toolchain".into(),
            args: vec!["build".into()],
            current_dir: "/workspace".into(),
            artifact_path: "/workspace/out/component.wasm".into(),
            metadata: ArtifactMetadata {
                name: "demo".into(),
                version: "1.0.0".into(),
                route_hints: vec!["file://**/*.rs/symbols".into()],
                dependencies: Vec::new(),
            },
        };
        assert!(request.validate().is_ok());
        assert!(
            BuildRequest {
                program: String::new(),
                ..request
            }
            .validate()
            .is_err()
        );
        assert!(BuildLimits::default().timeout > Duration::ZERO);
        assert!(BuildLimits::default().max_artifact_bytes > 0);
    }
}
