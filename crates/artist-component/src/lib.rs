//! Component artifact validation and activation preparation.
//!
//! Source builds belong behind a separate build service. This crate deals with
//! the stable artifact boundary consumed by the runtime.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, anyhow};
use artist_kernel::ResourceUri;
use artist_wasm::{
    Extension, ExtensionClass, ExtensionDependency, ExtensionMetadata, PreparedGeneration, Runtime,
};
use sha2::{Digest, Sha256};
use wasmtime::Engine;

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
}

impl CommandBuildService {
    pub fn new(engine: Engine) -> Self {
        Self::with_limits(engine, BuildLimits::default())
    }

    pub fn with_limits(engine: Engine, limits: BuildLimits) -> Self {
        Self { engine, limits }
    }
}

#[async_trait::async_trait]
impl SourceBuildService for CommandBuildService {
    async fn build(&self, request: BuildRequest) -> Result<BuildOutput, BuildError> {
        request.validate().map_err(BuildError::Invalid)?;
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
        Ok(BuildOutput {
            artifact,
            metadata: request.metadata,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactMetadata {
    pub name: String,
    pub version: String,
    pub claims: Vec<String>,
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
        for claim in &self.claims {
            claim
                .parse::<ResourceUri>()
                .with_context(|| format!("invalid component claim {claim}"))?;
        }
        ExtensionMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
            claims: self.claims.clone(),
            dependencies: self.dependencies.clone(),
        }
        .validate()?;
        Ok(())
    }

    fn runtime_metadata(&self) -> ExtensionMetadata {
        ExtensionMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
            claims: self.claims.clone(),
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
        (export "artist:nouns/namespace@1.0.0" (core module $m))
    )"#;

    #[test]
    fn validates_and_hashes_component_artifacts() {
        let engine = build_engine().unwrap();
        let artifact = ComponentArtifact::from_bytes(&engine, NOUN_COMPONENT.as_bytes()).unwrap();
        assert!(artifact.digest().len() == 64);
        assert!(artifact.class().noun);
        assert!(!artifact.class().verb);
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
                    claims: vec!["demo:///".into()],
                    dependencies: Vec::new(),
                },
            )
            .unwrap();
        let handle = runtime.activate(prepared).await.unwrap();
        assert_eq!(handle.name(), "demo");
    }

    #[test]
    fn rejects_invalid_claims_before_runtime_prepare() {
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
                        name: "demo".into(),
                        version: "1.0.0".into(),
                        claims: vec!["files:///../bad".into()],
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
                claims: vec!["demo:///".into()],
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
